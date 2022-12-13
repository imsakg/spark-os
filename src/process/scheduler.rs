//! Exclusive-owner single-core scheduling decisions, not hardware switching.

use super::{ProcessError, ProcessId, ProcessState, ProcessTable, RunQueue, WaitChannel};

/// A tick reports policy only; it never switches CPU context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickOutcome {
    /// No process was running during this tick.
    Idle,
    /// The running process still has budget.
    Continue(ProcessId),
    /// The caller should arrange a safe scheduling point.
    QuantumExpired(ProcessId),
}

/// Allocation-free cooperative scheduler. Mutate only through an exclusive owner.
pub struct Scheduler<const N: usize> {
    table: ProcessTable<N>,
    ready: RunQueue<N>,
    current: Option<ProcessId>,
    quantum: u32,
    remaining: u32,
}

impl<const N: usize> Scheduler<N> {
    /// Construct an empty scheduler.
    pub const fn new() -> Self {
        Self {
            table: ProcessTable::new(),
            ready: RunQueue::new(),
            current: None,
            quantum: 4,
            remaining: 0,
        }
    }

    /// Construct a scheduler with a checked, nonzero tick budget.
    pub fn with_quantum(quantum: u32) -> Result<Self, ProcessError> {
        if quantum == 0 {
            return Err(ProcessError::InvalidQuantum);
        }
        let mut scheduler = Self::new();
        scheduler.quantum = quantum;
        Ok(scheduler)
    }

    /// Remaining ticks in the current process's slice, or zero while idle.
    pub const fn remaining_quantum(&self) -> u32 {
        self.remaining
    }

    /// Consume one tick without automatically changing the running process.
    pub fn tick(&mut self) -> Result<TickOutcome, ProcessError> {
        match self.current {
            None => Ok(TickOutcome::Idle),
            Some(id) => {
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    Ok(TickOutcome::QuantumExpired(id))
                } else {
                    Ok(TickOutcome::Continue(id))
                }
            }
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
                self.remaining = self.quantum;
            }
        }
        self.current
    }

    /// Block on an event and immediately select another ready process if possible.
    /// The caller must atomically coordinate condition checks and event delivery.
    pub fn block_current(
        &mut self,
        channel: WaitChannel,
    ) -> Result<Option<ProcessId>, ProcessError> {
        let id = self.stop_current(ProcessState::Blocked)?;
        self.table.get_mut(id).unwrap().wait = Some(channel);
        Ok(self.dispatch())
    }

    /// Make one event-blocked process ready without implicitly dispatching it.
    pub fn wake(&mut self, id: ProcessId) -> Result<(), ProcessError> {
        if self.table.get(id)?.state() != ProcessState::Blocked {
            return Err(ProcessError::InvalidTransition);
        }
        self.table.transition(id, ProcessState::Ready)?;
        self.table.get_mut(id).unwrap().wait = None;
        self.ready.enqueue(id).unwrap();
        Ok(())
    }

    /// Wake the lowest-identity waiter matching this channel, if any.
    pub fn wake_one(&mut self, channel: WaitChannel) -> Option<ProcessId> {
        let id = self
            .table
            .slots
            .iter()
            .flatten()
            .filter(|p| p.state == ProcessState::Blocked && p.wait == Some(channel))
            .map(|p| p.id)
            .min()?;
        self.wake(id).unwrap();
        Some(id)
    }

    /// Wake every matching waiter in ascending identity order; return the count.
    pub fn wake_all(&mut self, channel: WaitChannel) -> usize {
        let mut count = 0;
        while self.wake_one(channel).is_some() {
            count += 1;
        }
        count
    }

    fn stop_current(&mut self, next: ProcessState) -> Result<ProcessId, ProcessError> {
        let id = self.current.ok_or(ProcessError::NoCurrent)?;
        self.table.transition(id, next)?;
        self.current = None;
        self.remaining = 0;
        Ok(id)
    }

    /// Voluntarily donate the unused slice and rejoin at the ready queue tail.
    /// Unlike `reschedule`, yielding while idle is an error with no side effects.
    pub fn yield_current(&mut self) -> Result<Option<ProcessId>, ProcessError> {
        let id = self.stop_current(ProcessState::Ready)?;
        self.ready.enqueue(id).unwrap();
        Ok(self.dispatch())
    }

    /// Rotate the running task to the queue tail and dispatch the oldest ready.
    /// This changes metadata only; the caller must implement hardware switching.
    pub fn reschedule(&mut self) -> Option<ProcessId> {
        if self.current.is_some() {
            let id = self.stop_current(ProcessState::Ready).unwrap();
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
