use super::{CachePolicy, FrameExtent, MemoryError, PagePermissions, PageRange, PhysAddr};

/// Checked software mapping of contiguous virtual pages to physical frames.
/// This value does not own frames or install a hardware page table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mapping {
    range: PageRange,
    physical: FrameExtent,
    permissions: PagePermissions,
    cache: CachePolicy,
}
impl Mapping {
    /// Create a nonempty mapping with representable physical and virtual ends.
    pub fn new(
        range: PageRange,
        physical: PhysAddr,
        permissions: PagePermissions,
        cache: CachePolicy,
    ) -> Result<Self, MemoryError> {
        if cache == CachePolicy::Device && permissions.executable() {
            return Err(MemoryError::InvalidPermissions);
        }
        Ok(Self {
            range,
            physical: FrameExtent::new(physical, range.page_count())?,
            permissions,
            cache,
        })
    }
    /// Checked virtual page interval.
    pub const fn range(self) -> PageRange {
        self.range
    }
    /// First physical frame address.
    pub const fn physical_start(self) -> PhysAddr {
        self.physical.start()
    }
    /// Checked page permissions, shared across the extent.
    pub const fn permissions(self) -> PagePermissions {
        self.permissions
    }
    /// Intended hardware memory cache policy.
    pub const fn cache(self) -> CachePolicy {
        self.cache
    }
}

/// Fixed-capacity software mapping plan sorted by virtual start address.
/// Callers supply synchronization and all later MMU/TLB/cache operations.
pub struct MappingTable<const N: usize> {
    entries: [Option<Mapping>; N],
    len: usize,
}
impl<const N: usize> MappingTable<N> {
    /// Create an empty table without runtime allocation.
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            len: 0,
        }
    }
    /// Number of stored mapping extents.
    pub const fn len(&self) -> usize {
        self.len
    }
    /// Whether there are no mappings.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Iterate checked extents in ascending virtual-address order.
    pub fn mappings(&self) -> impl Iterator<Item = Mapping> + '_ {
        self.entries[..self.len].iter().filter_map(|entry| *entry)
    }
    /// Insert a virtual extent without overlap or partial mutation on failure.
    pub fn map(&mut self, mapping: Mapping) -> Result<(), MemoryError> {
        if self
            .mappings()
            .any(|entry| entry.range().overlaps(mapping.range()))
        {
            return Err(MemoryError::AlreadyUsed);
        }
        if self.len == N {
            return Err(MemoryError::Capacity);
        }
        let index = self
            .mappings()
            .take_while(|entry| entry.range().start() < mapping.range().start())
            .count();
        for position in (index..self.len).rev() {
            self.entries[position + 1] = self.entries[position];
        }
        self.entries[index] = Some(mapping);
        self.len += 1;
        Ok(())
    }
}
impl<const N: usize> Default for MappingTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
