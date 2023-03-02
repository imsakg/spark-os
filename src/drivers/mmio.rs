//! Register transport and an explicitly unsafe volatile MMIO mapping boundary.

/// Serialized 32-bit device-register accesses at byte offsets.
///
/// Backends must preserve the hardware access width. Ordering barriers and
/// device-memory attributes are the board's responsibility, not this trait's.
pub trait RegisterIo {
    /// Read one aligned 32-bit register.
    fn read32(&mut self, offset: usize) -> u32;
    /// Write one aligned 32-bit register (not a read-modify-write).
    fn write32(&mut self, offset: usize, value: u32);
}

/// Invalid mapping metadata; construction never probes the physical address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmioError {
    /// Null, unaligned, empty, non-word-sized, or overflowing span.
    InvalidRegion,
}

/// Exclusive volatile transport over a caller-provided device-memory span.
///
/// This type deliberately has no `Send` or `Sync` implementation.
pub struct VolatileMmio {
    base: *mut u8,
    bytes: usize,
}

impl VolatileMmio {
    /// Validate mapping metadata and establish a volatile access boundary.
    ///
    /// # Safety
    /// For this transport's entire lifetime, `base..base+bytes` must remain a
    /// live, exclusively owned device mapping (or exclusive test RAM), with
    /// 32-bit register accesses permitted throughout. The caller must configure
    /// correct memory attributes, access privileges, board addresses, and any
    /// required architecture barriers. A physical address alone is not proof
    /// of a valid mapping. No references into the span may alias an access.
    pub unsafe fn from_raw(base: *mut u8, bytes: usize) -> Result<Self, MmioError> {
        if base.is_null()
            || base as usize % 4 != 0
            || bytes == 0
            || bytes % 4 != 0
            || (base as usize).checked_add(bytes).is_none()
            || bytes > isize::MAX as usize
        {
            return Err(MmioError::InvalidRegion);
        }
        Ok(Self { base, bytes })
    }

    fn register(&self, offset: usize) -> *mut u32 {
        assert!(
            offset % 4 == 0 && offset.checked_add(4).map_or(false, |end| end <= self.bytes),
            "MMIO offset outside aligned mapping"
        );
        // Safety: construction establishes a live span; the assertion bounds
        // the byte offset and guarantees alignment before pointer arithmetic.
        unsafe { self.base.add(offset).cast() }
    }
}

impl RegisterIo for VolatileMmio {
    fn read32(&mut self, offset: usize) -> u32 {
        // Safety: the constructor contract and `register` bounds apply.
        unsafe { core::ptr::read_volatile(self.register(offset)) }
    }
    fn write32(&mut self, offset: usize, value: u32) {
        // Safety: exclusive access is part of the constructor contract.
        unsafe { core::ptr::write_volatile(self.register(offset), value) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volatile_backend_reads_and_writes_aligned_test_ram() {
        // This is RAM, not a hardware qualification test.
        let mut ram = [7_u32, 0, 0, 0];
        let mut io = unsafe { VolatileMmio::from_raw(ram.as_mut_ptr().cast(), 16) }.unwrap();
        assert_eq!(io.read32(0), 7);
        io.write32(12, 0x12345678);
        assert_eq!(io.read32(12), 0x12345678);
        assert_eq!(ram[3], 0x12345678);
    }
    #[test]
    fn invalid_regions_are_rejected_without_access() {
        assert!(matches!(
            unsafe { VolatileMmio::from_raw(core::ptr::null_mut(), 4) },
            Err(MmioError::InvalidRegion)
        ));
        assert!(matches!(
            unsafe { VolatileMmio::from_raw(1_usize as *mut u8, 4) },
            Err(MmioError::InvalidRegion)
        ));
        let mut word = 0_u32;
        assert!(matches!(
            unsafe { VolatileMmio::from_raw((&mut word as *mut u32).cast(), 0) },
            Err(MmioError::InvalidRegion)
        ));
        assert!(matches!(
            unsafe { VolatileMmio::from_raw((&mut word as *mut u32).cast(), 3) },
            Err(MmioError::InvalidRegion)
        ));
    }
    #[test]
    #[should_panic(expected = "MMIO offset")]
    fn out_of_region_access_panics_before_dereference() {
        let mut word = 0_u32;
        let mut io = unsafe { VolatileMmio::from_raw((&mut word as *mut u32).cast(), 4) }.unwrap();
        io.read32(4);
    }
}
