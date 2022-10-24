use super::MemoryError;
use core::ops::Range;

fn checked_range(capacity: usize, offset: usize, len: usize) -> Result<Range<usize>, MemoryError> {
    let end = offset.checked_add(len).ok_or(MemoryError::Overflow)?;
    if end > capacity {
        return Err(MemoryError::OutOfRange);
    }
    Ok(offset..end)
}

/// Zero initialized bytes after checking offset arithmetic and the entire range.
/// Empty ranges at the slice end are allowed; failure leaves all bytes unchanged.
/// This is ordinary RAM access, not volatile MMIO or a cryptographic secret wipe.
pub fn zero_bytes(destination: &mut [u8], offset: usize, len: usize) -> Result<(), MemoryError> {
    let range = checked_range(destination.len(), offset, len)?;
    destination[range].fill(0);
    Ok(())
}

/// Copy initialized, nonaliasing slices after validating both complete ranges.
/// Offsets are in bytes; empty ranges at either slice end are valid. Failure is
/// atomic. Use `move_bytes` for overlap within one slice; this is not MMIO access.
pub fn copy_bytes(
    destination: &mut [u8],
    destination_offset: usize,
    source: &[u8],
    source_offset: usize,
    len: usize,
) -> Result<(), MemoryError> {
    let destination_range = checked_range(destination.len(), destination_offset, len)?;
    let source_range = checked_range(source.len(), source_offset, len)?;
    destination[destination_range].copy_from_slice(&source[source_range]);
    Ok(())
}

/// Move bytes within one initialized slice with memmove-style overlap semantics.
/// Source offset precedes destination offset in the arguments. Both ranges are
/// checked before any write; empty ranges at the slice end are valid.
pub fn move_bytes(
    bytes: &mut [u8],
    source_offset: usize,
    destination_offset: usize,
    len: usize,
) -> Result<(), MemoryError> {
    let source_range = checked_range(bytes.len(), source_offset, len)?;
    checked_range(bytes.len(), destination_offset, len)?;
    bytes.copy_within(source_range, destination_offset);
    Ok(())
}
