use super::{
    CachePolicy, FrameExtent, MemoryError, PagePermissions, PageRange, PhysAddr, VirtAddr,
};

/// Requested access kind for software permission checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessType {
    /// Read initialized mapped bytes.
    Read,
    /// Write mapped bytes.
    Write,
    /// Execute at the explicitly selected privilege level.
    Execute,
}

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
    /// Translate a byte address, preserving the offset across the whole extent.
    pub fn translate(self, address: VirtAddr) -> Result<PhysAddr, MemoryError> {
        if !self.range.contains(address) {
            return Err(MemoryError::NotMapped);
        }
        self.physical_start()
            .checked_add(address.get() - self.range.start().get())
    }
    fn check_access(self, access: AccessType, from_user: bool) -> Result<(), MemoryError> {
        let allowed = match access {
            AccessType::Read => true,
            AccessType::Write => self.permissions.writable(),
            AccessType::Execute => {
                self.permissions.executable() && self.permissions.user() == from_user
            }
        };
        if !allowed || (from_user && !self.permissions.user()) {
            return Err(MemoryError::PermissionDenied);
        }
        Ok(())
    }
    fn check_alias(self, other: Self) -> Result<(), MemoryError> {
        if self.physical.start() < other.physical.end()
            && other.physical.start() < self.physical.end()
        {
            if self.cache != other.cache {
                return Err(MemoryError::InvalidAttributes);
            }
            if self.permissions.user() != other.permissions.user()
                || (self.permissions.writable() && other.permissions.executable())
                || (other.permissions.writable() && self.permissions.executable())
            {
                return Err(MemoryError::InvalidPermissions);
            }
        }
        Ok(())
    }
    /// Create a nonempty mapping with representable physical and virtual ends.
    pub fn new(
        range: PageRange,
        physical: PhysAddr,
        permissions: PagePermissions,
        cache: CachePolicy,
    ) -> Result<Self, MemoryError> {
        if cache == CachePolicy::Device && (permissions.executable() || permissions.user()) {
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
/// Physical aliases within this table must share cache policy and user access,
/// and no physical byte may be writable through one alias and executable through another.
/// Mappings in other tables or installed by platform code are not inspected.
pub struct MappingTable<const N: usize> {
    entries: [Option<Mapping>; N],
    len: usize,
}
impl<const N: usize> MappingTable<N> {
    /// Find the unique extent containing a virtual byte address.
    pub fn lookup(&self, address: VirtAddr) -> Option<Mapping> {
        self.mappings()
            .find(|entry| entry.range().contains(address))
    }
    /// Translate a byte address without claiming any access permission.
    pub fn translate(&self, address: VirtAddr) -> Result<PhysAddr, MemoryError> {
        self.lookup(address)
            .ok_or(MemoryError::NotMapped)?
            .translate(address)
    }
    /// Translate after checking access and EL0/EL1 execution restrictions.
    /// This software check does not model PAN, privilege overrides, or live MMU state.
    pub fn resolve(
        &self,
        address: VirtAddr,
        access: AccessType,
        from_user: bool,
    ) -> Result<PhysAddr, MemoryError> {
        let mapping = self.lookup(address).ok_or(MemoryError::NotMapped)?;
        mapping.check_access(access, from_user)?;
        mapping.translate(address)
    }
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
        for entry in self.mappings() {
            mapping.check_alias(entry)?;
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
