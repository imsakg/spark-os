use super::{MemoryError, PhysAddr, PAGE_SIZE};

/// First-fit physical-frame allocator backed by two caller-owned bitmaps.
///
/// Construction clears both bitmaps after validating all inputs. Addresses are
/// bookkeeping only: this allocator never reads or writes physical RAM.
pub struct BitmapFrames<'a> {
    base: PhysAddr,
    count: usize,
    used: &'a mut [u64],
    reserved: &'a mut [u64],
}

/// Snapshot of frame counts; every field is measured in 4 KiB frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameStats {
    /// Managed frames including reservations.
    pub total: usize,
    /// Frames allocated to clients.
    pub allocated: usize,
    /// Permanently excluded frames.
    pub reserved: usize,
    /// Frames available for allocation.
    pub free: usize,
    /// Largest physically contiguous free interval.
    pub largest_free_run: usize,
}

/// A nonempty checked extent of physically contiguous 4 KiB frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameExtent {
    start: PhysAddr,
    count: usize,
}

impl FrameExtent {
    /// Describe an extent without allocating it or claiming ownership.
    pub fn new(start: PhysAddr, count: usize) -> Result<Self, MemoryError> {
        if start.page_offset() != 0 {
            return Err(MemoryError::Misaligned);
        }
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }
        let bytes = count.checked_mul(PAGE_SIZE).ok_or(MemoryError::Overflow)?;
        start.checked_add(bytes)?;
        Ok(Self { start, count })
    }
    /// First physical frame address.
    pub const fn start(self) -> PhysAddr {
        self.start
    }
    /// Exclusive physical end, validated at construction.
    pub fn end(self) -> PhysAddr {
        PhysAddr::new(self.start.get() + self.byte_len())
    }
    /// Extent length in frames.
    pub const fn frame_count(self) -> usize {
        self.count
    }
    /// Extent length in bytes.
    pub const fn byte_len(self) -> usize {
        self.count * PAGE_SIZE
    }
}

impl<'a> BitmapFrames<'a> {
    /// Recompute accounting and fragmentation without trusting bitmap tail bits.
    pub fn stats(&self) -> FrameStats {
        let mut stats = FrameStats {
            total: self.count,
            allocated: 0,
            reserved: 0,
            free: 0,
            largest_free_run: 0,
        };
        let mut run = 0;
        for index in 0..self.count {
            let mask = 1u64 << (index % 64);
            if self.reserved[index / 64] & mask != 0 {
                stats.reserved += 1;
                run = 0;
            } else if self.used[index / 64] & mask != 0 {
                stats.allocated += 1;
                run = 0;
            } else {
                stats.free += 1;
                run += 1;
                stats.largest_free_run = core::cmp::max(stats.largest_free_run, run);
            }
        }
        stats
    }
    /// Create a nonempty frame pool; each bitmap needs ceil(count / 64) words.
    pub fn new(
        base: PhysAddr,
        count: usize,
        used: &'a mut [u64],
        reserved: &'a mut [u64],
    ) -> Result<Self, MemoryError> {
        if base.page_offset() != 0 {
            return Err(MemoryError::Misaligned);
        }
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }
        let bytes = count.checked_mul(PAGE_SIZE).ok_or(MemoryError::Overflow)?;
        base.checked_add(bytes)?;
        let words = count / 64 + usize::from(count % 64 != 0);
        if used.len() < words || reserved.len() < words {
            return Err(MemoryError::Capacity);
        }
        used.fill(0);
        reserved.fill(0);
        Ok(Self {
            base,
            count,
            used,
            reserved,
        })
    }
    /// First managed physical address.
    pub const fn base(&self) -> PhysAddr {
        self.base
    }
    /// Number of managed frames, excluding unused bitmap tail bits.
    pub const fn frame_count(&self) -> usize {
        self.count
    }
    fn index(&self, address: PhysAddr) -> Result<usize, MemoryError> {
        if address.page_offset() != 0 {
            return Err(MemoryError::Misaligned);
        }
        let offset = address
            .get()
            .checked_sub(self.base.get())
            .ok_or(MemoryError::OutOfRange)?;
        let index = offset / PAGE_SIZE;
        if index >= self.count {
            return Err(MemoryError::OutOfRange);
        }
        Ok(index)
    }
    fn occupied(&self, index: usize) -> bool {
        (self.used[index / 64] | self.reserved[index / 64]) & (1u64 << (index % 64)) != 0
    }
    fn address(&self, index: usize) -> PhysAddr {
        PhysAddr::new(self.base.get() + index * PAGE_SIZE)
    }
    /// Return whether a frame is allocated or permanently reserved.
    pub fn is_allocated(&self, address: PhysAddr) -> Result<bool, MemoryError> {
        Ok(self.occupied(self.index(address)?))
    }
    fn indices(
        &self,
        address: PhysAddr,
        count: usize,
    ) -> Result<core::ops::Range<usize>, MemoryError> {
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }
        let start = self.index(address)?;
        let end = start.checked_add(count).ok_or(MemoryError::Overflow)?;
        if end > self.count {
            return Err(MemoryError::OutOfRange);
        }
        Ok(start..end)
    }

    /// Permanently exclude an entirely free extent, committing only on success.
    pub fn reserve(&mut self, address: PhysAddr, count: usize) -> Result<(), MemoryError> {
        let indices = self.indices(address, count)?;
        if indices.clone().any(|index| self.occupied(index)) {
            return Err(MemoryError::AlreadyUsed);
        }
        for index in indices {
            self.reserved[index / 64] |= 1u64 << (index % 64);
        }
        Ok(())
    }

    /// Release an allocated frame, rejecting invalid and duplicate frees.
    pub fn free(&mut self, address: PhysAddr) -> Result<(), MemoryError> {
        let index = self.index(address)?;
        let mask = 1u64 << (index % 64);
        if self.reserved[index / 64] & mask != 0 {
            return Err(MemoryError::Reserved);
        }
        if self.used[index / 64] & mask == 0 {
            return Err(MemoryError::DoubleFree);
        }
        self.used[index / 64] &= !mask;
        Ok(())
    }

    /// Allocate the lowest free frame, or fail without changing the pool.
    pub fn allocate(&mut self) -> Result<PhysAddr, MemoryError> {
        self.allocate_contiguous(1).map(FrameExtent::start)
    }

    /// Allocate a first-fit contiguous extent without partial claims on failure.
    pub fn allocate_contiguous(&mut self, count: usize) -> Result<FrameExtent, MemoryError> {
        self.allocate_aligned(count, PAGE_SIZE)
    }

    /// Allocate a contiguous extent aligned in physical bytes, not pool indices.
    /// Alignment must be a power of two no smaller than one frame.
    pub fn allocate_aligned(
        &mut self,
        count: usize,
        alignment: usize,
    ) -> Result<FrameExtent, MemoryError> {
        if alignment < PAGE_SIZE || !alignment.is_power_of_two() {
            return Err(MemoryError::InvalidAlignment);
        }
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }
        count.checked_mul(PAGE_SIZE).ok_or(MemoryError::Overflow)?;
        let mut run = 0;
        for index in 0..self.count {
            if self.occupied(index) {
                run = 0;
            } else if run != 0 || self.address(index).get() & (alignment - 1) == 0 {
                run += 1;
            }
            if run == count {
                let start = index + 1 - count;
                for allocated in start..=index {
                    self.used[allocated / 64] |= 1u64 << (allocated % 64);
                }
                return FrameExtent::new(self.address(start), count);
            }
        }
        Err(MemoryError::OutOfMemory)
    }
}
