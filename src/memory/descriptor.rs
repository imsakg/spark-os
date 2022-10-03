use super::{CachePolicy, MemoryError, PhysAddr};

/// Readable page permissions with write-xor-execute enforced at construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagePermissions {
    writable: bool,
    executable: bool,
    user: bool,
}
impl PagePermissions {
    /// Create readable permissions; writable executable pages are forbidden.
    pub fn new(writable: bool, executable: bool, user: bool) -> Result<Self, MemoryError> {
        if writable && executable {
            return Err(MemoryError::InvalidPermissions);
        }
        Ok(Self {
            writable,
            executable,
            user,
        })
    }
    /// Whether writes are permitted.
    pub const fn writable(self) -> bool {
        self.writable
    }
    /// Whether execution is permitted at the selected privilege level.
    pub const fn executable(self) -> bool {
        self.executable
    }
    /// Whether EL0 access is permitted; executable EL0 pages remain PXN.
    pub const fn user(self) -> bool {
        self.user
    }
}

fn descriptor_address(address: PhysAddr) -> Result<u64, MemoryError> {
    if address.page_offset() != 0 {
        return Err(MemoryError::Misaligned);
    }
    let value = address.get() as u64;
    if value >= (1u64 << 48) {
        return Err(MemoryError::AddressWidth);
    }
    Ok(value)
}

/// Pure AArch64 stage-1 level-3 descriptor for 4 KiB pages and 48-bit PA.
/// Does not install page tables, alter registers, or enable the MMU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageDescriptor(u64);
impl PageDescriptor {
    /// Encode a readable AF-set leaf. MAIR indices must be configured by the
    /// platform: 0 = normal write-back, 1 = normal non-cacheable, 2 = Device-nGnRnE.
    /// Normal memory is inner-shareable; devices are outer-shareable.
    pub fn new(
        address: PhysAddr,
        permissions: PagePermissions,
        cache: CachePolicy,
    ) -> Result<Self, MemoryError> {
        let address = descriptor_address(address)?;
        if cache == CachePolicy::Device && (permissions.executable() || permissions.user()) {
            return Err(MemoryError::InvalidPermissions);
        }
        let (attribute, shareability) = match cache {
            CachePolicy::WriteBack => (0, 3),
            CachePolicy::NonCacheable => (1, 3),
            CachePolicy::Device => (2, 2),
        };
        let mut bits = address | 3 | (attribute << 2) | (shareability << 8) | (1 << 10);
        if permissions.user() {
            bits |= (1 << 6) | (1 << 11);
        }
        if !permissions.writable() {
            bits |= 1 << 7;
        }
        if permissions.user() || !permissions.executable() {
            bits |= 1 << 53;
        }
        if !permissions.user() || !permissions.executable() {
            bits |= 1 << 54;
        }
        Ok(Self(bits))
    }
    /// Encoded descriptor bits, not a live hardware mapping.
    pub const fn bits(self) -> u64 {
        self.0
    }
    /// Extract the physical output frame address.
    pub fn output_address(self) -> PhysAddr {
        PhysAddr::new((self.0 & 0x0000_ffff_ffff_f000) as usize)
    }
}

/// Pure next-level AArch64 table descriptor without hierarchical restrictions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableDescriptor(u64);
impl TableDescriptor {
    /// Encode an aligned next-table PA; the caller owns and initializes the table.
    pub fn new(address: PhysAddr) -> Result<Self, MemoryError> {
        Ok(Self(descriptor_address(address)? | 3))
    }
    /// Encoded table descriptor bits.
    pub const fn bits(self) -> u64 {
        self.0
    }
}
