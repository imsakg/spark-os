#![no_std]
//! Allocation-free, architecture-neutral kernel services.
//!
//! The bare-metal binary uses the same library that host tests exercise.

/// Kernel ABI revision for the initial service interfaces.
pub const ABI_VERSION: u32 = 1;

pub mod drivers;
pub mod memory;
pub mod modules;
pub mod process;

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    #[test]
    fn kernel_abi_is_versioned() {
        assert_eq!(super::ABI_VERSION, 1);
    }
}
