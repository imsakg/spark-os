//! Process lifecycle and cooperative scheduling.

/// A nonzero process identifier. Zero is reserved for non-process kernel work.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcessId(u64);

impl ProcessId {
    /// Construct an identifier, rejecting reserved identifier zero.
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Return the numeric identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Lifecycle state of a process in the single-core scheduling model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    /// Eligible for dispatch.
    Ready,
    /// Selected as the currently running process.
    Running,
    /// Waiting for an explicit event.
    Blocked,
    /// Waiting for a clock deadline.
    Sleeping,
    /// Exited, with metadata retained until reaped.
    Zombie,
}

impl ProcessState {
    /// Whether the process can execute without a wakeup.
    pub const fn is_runnable(self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }

    /// Whether the process has finished execution.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Zombie)
    }
}

/// A bounded process-table operation failed without allocating memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessError {
    /// All process slots are occupied.
    TableFull,
    /// No live or retained process has this identifier.
    NotFound,
}

/// Allocation-free metadata for one process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Process {
    id: ProcessId,
    state: ProcessState,
}

impl Process {
    /// Return the process identity.
    pub const fn id(&self) -> ProcessId {
        self.id
    }

    /// Return the current lifecycle state.
    pub const fn state(&self) -> ProcessState {
        self.state
    }
}

/// Fixed-capacity table with checked identifier lookup.
pub struct ProcessTable<const N: usize> {
    slots: [Option<Process>; N],
}

impl<const N: usize> ProcessTable<N> {
    /// Construct an empty table.
    pub const fn new() -> Self {
        Self { slots: [None; N] }
    }

    /// Maximum number of retained processes.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Number of occupied process slots.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Whether all slots are empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Allocate a ready process, failing atomically when full.
    pub fn spawn(&mut self) -> Result<ProcessId, ProcessError> {
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(ProcessError::TableFull)?;
        let id = ProcessId::new(index as u64 + 1).unwrap();
        self.slots[index] = Some(Process {
            id,
            state: ProcessState::Ready,
        });
        Ok(id)
    }

    /// Look up a process by identity, never by unchecked slot index.
    pub fn get(&self, id: ProcessId) -> Result<&Process, ProcessError> {
        self.slots
            .iter()
            .flatten()
            .find(|process| process.id == id)
            .ok_or(ProcessError::NotFound)
    }
}

impl<const N: usize> Default for ProcessTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
