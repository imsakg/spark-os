use super::{FrameExtent, MemoryError, PhysAddr, PAGE_SIZE};

/// Firmware or platform classification of physical memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryKind {
    /// RAM that may supply allocator frames after kernel exclusions.
    Usable,
    /// RAM unavailable to general allocation.
    Reserved,
    /// Memory-mapped device registers, never allocator memory.
    Device,
}

/// Intended cache behavior; platform code supplies matching MAIR values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CachePolicy {
    /// Normal write-back cacheable RAM.
    WriteBack,
    /// Normal non-cacheable RAM.
    NonCacheable,
    /// Device-nGnRnE memory.
    Device,
}

/// Consistent physical memory classification and cache policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionAttributes {
    kind: MemoryKind,
    cache: CachePolicy,
}
impl RegionAttributes {
    /// Reject device cache modes on RAM and normal cache modes on devices.
    pub fn new(kind: MemoryKind, cache: CachePolicy) -> Result<Self, MemoryError> {
        if (kind == MemoryKind::Device) != (cache == CachePolicy::Device) {
            return Err(MemoryError::InvalidAttributes);
        }
        Ok(Self { kind, cache })
    }
    /// Firmware or platform memory classification.
    pub const fn kind(self) -> MemoryKind {
        self.kind
    }
    /// Intended cache policy.
    pub const fn cache(self) -> CachePolicy {
        self.cache
    }
    /// Whether this classification permits general frame allocation.
    pub fn is_allocatable(self) -> bool {
        self.kind == MemoryKind::Usable
    }
}

/// An aligned, nonempty physical region with checked attributes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    extent: FrameExtent,
    attributes: RegionAttributes,
}
impl MemoryRegion {
    /// Construct a page-aligned physical byte region; byte length must be aligned.
    pub fn new(
        start: PhysAddr,
        bytes: usize,
        attributes: RegionAttributes,
    ) -> Result<Self, MemoryError> {
        if bytes % PAGE_SIZE != 0 {
            return Err(MemoryError::Misaligned);
        }
        Ok(Self {
            extent: FrameExtent::new(start, bytes / PAGE_SIZE)?,
            attributes,
        })
    }
    /// First physical byte address.
    pub const fn start(self) -> PhysAddr {
        self.extent.start()
    }
    /// Exclusive end address.
    pub fn end(self) -> PhysAddr {
        self.extent.end()
    }
    /// Byte length.
    pub const fn byte_len(self) -> usize {
        self.extent.byte_len()
    }
    /// Checked region attributes.
    pub const fn attributes(self) -> RegionAttributes {
        self.attributes
    }
    /// Test byte membership using half-open bounds.
    pub fn contains(self, address: PhysAddr) -> bool {
        address >= self.start() && address < self.end()
    }
    /// Test physical overlap; adjacent regions do not overlap.
    pub fn overlaps(self, other: Self) -> bool {
        self.start() < other.end() && other.start() < self.end()
    }
}
