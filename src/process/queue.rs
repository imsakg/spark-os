//! Bounded FIFO ready identities.

use super::ProcessId;

/// Rejected run-queue insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    /// All queue entries are occupied.
    Full,
    /// The identity is already queued.
    Duplicate,
}

/// Fixed-capacity FIFO ring. Each identity appears at most once.
pub struct RunQueue<const N: usize> {
    entries: [Option<ProcessId>; N],
    head: usize,
    len: usize,
}

impl<const N: usize> RunQueue<N> {
    /// Construct an empty queue, including for zero capacity.
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            head: 0,
            len: 0,
        }
    }

    /// Number of queued identities.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no identity is queued.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate from oldest to newest admission.
    pub fn iter(&self) -> impl Iterator<Item = ProcessId> + '_ {
        (0..self.len).map(move |offset| self.entries[(self.head + offset) % N].unwrap())
    }

    /// Append once, preserving the queue on full/duplicate rejection.
    pub fn enqueue(&mut self, id: ProcessId) -> Result<(), QueueError> {
        if self.iter().any(|entry| entry == id) {
            return Err(QueueError::Duplicate);
        }
        if self.len == N {
            return Err(QueueError::Full);
        }
        self.entries[(self.head + self.len) % N] = Some(id);
        self.len += 1;
        Ok(())
    }

    /// Remove and return the oldest identity.
    pub fn pop(&mut self) -> Option<ProcessId> {
        if self.is_empty() {
            return None;
        }
        let value = self.entries[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        value
    }

    /// Remove a specific identity without disturbing the remaining FIFO order.
    pub fn remove(&mut self, id: ProcessId) -> bool {
        let offset = match self.iter().position(|entry| entry == id) {
            Some(offset) => offset,
            None => return false,
        };
        for next in offset + 1..self.len {
            self.entries[(self.head + next - 1) % N] = self.entries[(self.head + next) % N];
        }
        self.entries[(self.head + self.len - 1) % N] = None;
        self.len -= 1;
        true
    }
}

impl<const N: usize> Default for RunQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}
