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

impl<'a> BitmapFrames<'a> {
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
    /// Allocate the lowest free frame, or fail without changing the pool.
    pub fn allocate(&mut self) -> Result<PhysAddr, MemoryError> {
        for index in 0..self.count {
            if !self.occupied(index) {
                self.used[index / 64] |= 1u64 << (index % 64);
                return Ok(self.address(index));
            }
        }
        Err(MemoryError::OutOfMemory)
    }
}
