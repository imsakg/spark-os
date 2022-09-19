use super::{align_up, MemoryError};
use core::alloc::Layout;

/// Safe early-boot byte arena over initialized, caller-owned writable memory.
///
/// Allocations borrow the arena mutably, preventing rollback or another
/// allocation while a returned slice remains in use. This is not GlobalAlloc.
pub struct BumpArena<'a> {
    backing: &'a mut [u8],
    cursor: usize,
    generation: usize,
}

/// Arena position scoped to the original live backing slice and rollback epoch.
/// Do not retain checkpoints after destroying and recreating their arena.
#[derive(Clone, Copy, Debug)]
pub struct BumpCheckpoint {
    base: usize,
    capacity: usize,
    cursor: usize,
    generation: usize,
}

impl<'a> BumpArena<'a> {
    /// Inspect initialized backing bytes without permitting mutation.
    pub fn contents(&self) -> &[u8] {
        self.backing
    }
    /// Capture a position. A successful rollback invalidates every old checkpoint.
    pub fn checkpoint(&self) -> BumpCheckpoint {
        BumpCheckpoint {
            base: self.backing.as_ptr() as usize,
            capacity: self.capacity(),
            cursor: self.cursor,
            generation: self.generation,
        }
    }
    /// Zero discarded allocations and padding, then restore a current checkpoint.
    /// This is ordinary initialization, not a volatile cryptographic secret wipe.
    pub fn rollback(&mut self, checkpoint: BumpCheckpoint) -> Result<(), MemoryError> {
        if checkpoint.base != self.backing.as_ptr() as usize
            || checkpoint.capacity != self.capacity()
            || checkpoint.generation != self.generation
            || checkpoint.cursor > self.cursor
        {
            return Err(MemoryError::InvalidCheckpoint);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(MemoryError::Overflow)?;
        self.backing[checkpoint.cursor..self.cursor].fill(0);
        self.cursor = checkpoint.cursor;
        self.generation = generation;
        Ok(())
    }
    /// Create an empty arena without modifying its backing bytes.
    pub fn new(backing: &'a mut [u8]) -> Self {
        Self {
            backing,
            cursor: 0,
            generation: 0,
        }
    }
    /// Total backing capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.backing.len()
    }
    /// Consumed bytes, including alignment padding.
    pub const fn used(&self) -> usize {
        self.cursor
    }
    /// Remaining backing bytes before alignment overhead.
    pub fn remaining(&self) -> usize {
        self.backing.len() - self.cursor
    }
    /// Allocate initialized bytes at the Layout's actual pointer alignment.
    /// Zero-size layouts are rejected. Failure does not consume any bytes.
    pub fn allocate(&mut self, layout: Layout) -> Result<&mut [u8], MemoryError> {
        if layout.size() == 0 {
            return Err(MemoryError::InvalidSize);
        }
        let base = self.backing.as_ptr() as usize;
        let next = base.checked_add(self.cursor).ok_or(MemoryError::Overflow)?;
        let aligned = align_up(next, layout.align())?;
        let start = aligned - base;
        let end = start
            .checked_add(layout.size())
            .ok_or(MemoryError::Overflow)?;
        if end > self.backing.len() {
            return Err(MemoryError::OutOfMemory);
        }
        self.cursor = end;
        Ok(&mut self.backing[start..end])
    }
}
