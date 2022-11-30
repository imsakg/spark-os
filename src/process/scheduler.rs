//! Exclusive-owner single-core scheduling decisions, not hardware switching.

use super::{ProcessError, ProcessId, ProcessState, ProcessTable, RunQueue};

/// Allocation-free cooperative scheduler. Mutate only through an exclusive owner.
pub struct Scheduler<const N: usize> {
    table: ProcessTable<N>,
    ready: RunQueue<N>,
    current: Option<ProcessId>,
}

impl<const N: usize> Scheduler<N> {
    /// Construct an empty scheduler.
    pub const fn new() -> Self {
        Self {
            table: ProcessTable::new(),
            ready: RunQueue::new(),
            current: None,
        }
    }

    /// Admit one new ready process. Rejection leaves the queue unchanged.
    pub fn spawn(&mut self) -> Result<ProcessId, ProcessError> {
        let id = self.table.spawn()?;
        // Queue capacity equals table capacity and only ready tasks are queued.
        self.ready.enqueue(id).unwrap();
        Ok(id)
    }

    /// Read-only access to retained process metadata.
    pub const fn processes(&self) -> &ProcessTable<N> {
        &self.table
    }

    /// Identity selected as running, if any.
    pub const fn current(&self) -> Option<ProcessId> {
        self.current
    }

    /// Number of ready processes, excluding the running process.
    pub const fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// Select the oldest ready process only when no process is running.
    pub fn dispatch(&mut self) -> Option<ProcessId> {
        if self.current.is_none() {
            if let Some(id) = self.ready.pop() {
                self.table.transition(id, ProcessState::Running).unwrap();
                self.current = Some(id);
            }
        }
        self.current
    }

    /// Rotate the running task to the queue tail and dispatch the oldest ready.
    /// This changes metadata only; the caller must implement hardware switching.
    pub fn reschedule(&mut self) -> Option<ProcessId> {
        if let Some(id) = self.current.take() {
            self.table.transition(id, ProcessState::Ready).unwrap();
            self.ready.enqueue(id).unwrap();
        }
        self.dispatch()
    }
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}
