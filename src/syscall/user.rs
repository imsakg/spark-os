//! A safe admitted-buffer boundary, never a raw userspace pointer dereference.

use super::Errno;
use core::ops::Range;

/// Exclusively borrowed storage admitted to one call's address-space boundary.
///
/// The base is an address label, not a pointer. This safe backing-slice model is
/// suitable for in-kernel calls and tests; a future EL0 adapter must first pin and
/// validate the caller's actual mappings. Never construct it over kernel secrets.
pub struct UserMemory<'a> {
    base: usize,
    storage: &'a mut [u8],
}

impl<'a> UserMemory<'a> {
    /// Admit one owned buffer, rejecting a wrapping labeled address span.
    pub fn new(base: usize, storage: &'a mut [u8]) -> Result<Self, Errno> {
        base.checked_add(storage.len()).ok_or(Errno::Fault)?;
        Ok(Self { base, storage })
    }

    fn range(&self, address: u64, length: u64) -> Result<Range<usize>, Errno> {
        let address = usize::try_from(address).map_err(|_| Errno::Fault)?;
        let length = usize::try_from(length).map_err(|_| Errno::Fault)?;
        let start = address.checked_sub(self.base).ok_or(Errno::Fault)?;
        let end = start.checked_add(length).ok_or(Errno::Fault)?;
        if end > self.storage.len() {
            return Err(Errno::Fault);
        }
        Ok(start..end)
    }

    /// Borrow readable bytes after checking the entire half-open range.
    pub fn read(&self, address: u64, length: u64) -> Result<&[u8], Errno> {
        Ok(&self.storage[self.range(address, length)?])
    }

    /// Borrow writable bytes after checking the entire range before mutation.
    pub fn writable(&mut self, address: u64, length: u64) -> Result<&mut [u8], Errno> {
        let range = self.range(address, length)?;
        Ok(&mut self.storage[range])
    }

    /// Copy output only after validation; invalid requests leave backing bytes intact.
    pub fn copy_out(&mut self, address: u64, source: &[u8]) -> Result<(), Errno> {
        self.writable(address, source.len() as u64)?
            .copy_from_slice(source);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syscall::Errno;
    #[test]
    fn buffer_ranges_allow_exact_end_but_never_overflow_or_cross_bounds() {
        let mut bytes = [1, 2, 3, 4];
        let mut memory = UserMemory::new(0x1000, &mut bytes).unwrap();
        assert_eq!(memory.read(0x1000, 4), Ok(&[1, 2, 3, 4][..]));
        assert_eq!(memory.read(0x1004, 0), Ok(&[][..]));
        for (address, length) in [(0xfff, 1), (0x1004, 1), (u64::MAX, 2), (0x1000, u64::MAX)] {
            assert_eq!(memory.read(address, length), Err(Errno::Fault));
        }
        assert_eq!(memory.copy_out(0x1003, &[8, 9]), Err(Errno::Fault));
        assert_eq!(memory.read(0x1000, 4), Ok(&[1, 2, 3, 4][..]));
        memory.copy_out(0x1002, &[8, 9]).unwrap();
        assert_eq!(memory.read(0x1000, 4), Ok(&[1, 2, 8, 9][..]));
        assert!(matches!(
            UserMemory::new(usize::MAX, &mut [0]),
            Err(Errno::Fault)
        ));
    }
}
