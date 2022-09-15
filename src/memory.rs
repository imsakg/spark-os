//! Physical memory and address-space management.

/// Checked memory-operation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    /// Alignment must be a nonzero power of two.
    InvalidAlignment,
    /// An address or byte count would wrap.
    Overflow,
    /// A page or frame address is not 4 KiB aligned.
    Misaligned,
    /// A nonempty operation was given zero elements.
    InvalidSize,
    /// Caller-provided fixed storage is too small.
    Capacity,
    /// No suitable free memory remains.
    OutOfMemory,
    /// An address or interval is outside the managed memory.
    OutOfRange,
    /// A released frame was already free.
    DoubleFree,
    /// A permanent reservation cannot be released as an allocation.
    Reserved,
    /// An allocation or reservation already occupies the requested interval.
    AlreadyUsed,
    /// Memory classification and cache behavior are inconsistent.
    InvalidAttributes,
}

/// Round down to a checked power-of-two alignment.
pub fn align_down(value: usize, alignment: usize) -> Result<usize, MemoryError> {
    if !alignment.is_power_of_two() {
        return Err(MemoryError::InvalidAlignment);
    }
    Ok(value & !(alignment - 1))
}

/// Round up without wrapping the address space.
pub fn align_up(value: usize, alignment: usize) -> Result<usize, MemoryError> {
    let down = align_down(value, alignment)?;
    if down == value {
        Ok(value)
    } else {
        down.checked_add(alignment).ok_or(MemoryError::Overflow)
    }
}

/// Check alignment, rejecting invalid alignment values.
pub fn is_aligned(value: usize, alignment: usize) -> Result<bool, MemoryError> {
    Ok(align_down(value, alignment)? == value)
}

mod address;
pub use address::{PhysAddr, VirtAddr, PAGE_SIZE};

mod range;
pub use range::PageRange;

mod frame;
pub use frame::{BitmapFrames, FrameExtent, FrameStats};

mod region;
pub use region::{CachePolicy, MemoryKind, MemoryRegion, RegionAttributes, RegionMap};

#[cfg(test)]
mod tests;
