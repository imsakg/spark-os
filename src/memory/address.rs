use super::{align_down, align_up, MemoryError};

/// Base page and physical frame size in bytes.
pub const PAGE_SIZE: usize = 4096;

macro_rules! address {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub struct $name(usize);

        impl $name {
            /// Wrap an integer without claiming hardware address validity.
            pub const fn new(value: usize) -> Self {
                Self(value)
            }
            /// Return the underlying byte address.
            pub const fn get(self) -> usize {
                self.0
            }
            /// Add a byte offset without wrapping.
            pub fn checked_add(self, bytes: usize) -> Result<Self, MemoryError> {
                self.0
                    .checked_add(bytes)
                    .map(Self)
                    .ok_or(MemoryError::Overflow)
            }
            /// Subtract a byte offset without wrapping.
            pub fn checked_sub(self, bytes: usize) -> Result<Self, MemoryError> {
                self.0
                    .checked_sub(bytes)
                    .map(Self)
                    .ok_or(MemoryError::Overflow)
            }
            /// Round toward zero to a checked alignment.
            pub fn align_down(self, alignment: usize) -> Result<Self, MemoryError> {
                align_down(self.0, alignment).map(Self)
            }
            /// Round upward to a checked alignment.
            pub fn align_up(self, alignment: usize) -> Result<Self, MemoryError> {
                align_up(self.0, alignment).map(Self)
            }
            /// Return the offset within a 4 KiB page.
            pub const fn page_offset(self) -> usize {
                self.0 & (PAGE_SIZE - 1)
            }
        }
    };
}

address!(
    PhysAddr,
    "A physical byte address, never implicitly converted to virtual."
);
address!(
    VirtAddr,
    "A virtual byte address; canonicality is a platform concern."
);
