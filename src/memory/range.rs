use super::{MemoryError, VirtAddr, PAGE_SIZE};

/// A checked, half-open interval of 4 KiB virtual pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageRange {
    start: VirtAddr,
    count: usize,
    end: VirtAddr,
}

impl PageRange {
    /// Construct an aligned range; the exclusive end must be representable.
    pub fn new(start: VirtAddr, count: usize) -> Result<Self, MemoryError> {
        if start.page_offset() != 0 {
            return Err(MemoryError::Misaligned);
        }
        let bytes = count.checked_mul(PAGE_SIZE).ok_or(MemoryError::Overflow)?;
        let end = start.checked_add(bytes)?;
        Ok(Self { start, count, end })
    }
    /// Cover a byte interval, rounding outward; an empty interval covers no pages.
    pub fn covering(start: VirtAddr, bytes: usize) -> Result<Self, MemoryError> {
        let aligned = start.align_down(PAGE_SIZE)?;
        if bytes == 0 {
            return Self::new(aligned, 0);
        }
        let end = start.checked_add(bytes)?.align_up(PAGE_SIZE)?;
        Self::new(aligned, (end.get() - aligned.get()) / PAGE_SIZE)
    }
    /// First page address.
    pub const fn start(self) -> VirtAddr {
        self.start
    }
    /// Exclusive end address.
    pub const fn end(self) -> VirtAddr {
        self.end
    }
    /// Number of pages.
    pub const fn page_count(self) -> usize {
        self.count
    }
    /// Number of bytes, checked at construction.
    pub const fn byte_len(self) -> usize {
        self.end.get() - self.start.get()
    }
    /// Whether the interval has no pages.
    pub const fn is_empty(self) -> bool {
        self.count == 0
    }
    /// Test byte membership, excluding the end address.
    pub fn contains(self, address: VirtAddr) -> bool {
        address >= self.start && address < self.end
    }
    /// Test intersection; adjacent and empty ranges do not overlap.
    pub fn overlaps(self, other: Self) -> bool {
        !self.is_empty() && !other.is_empty() && self.start < other.end && other.start < self.end
    }
    /// Iterate page starts in ascending order without allocation.
    pub fn pages(self) -> impl ExactSizeIterator<Item = VirtAddr> {
        (0..self.count).map(move |index| VirtAddr::new(self.start.get() + index * PAGE_SIZE))
    }
}
