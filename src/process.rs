//! Process lifecycle and cooperative scheduling.

mod policy;
mod queue;
pub use policy::{Priority, SchedulingPolicy, AGING_INTERVAL};
mod scheduler;
mod stats;
pub use queue::{QueueError, RunQueue};
pub use scheduler::{ScheduleDecision, Scheduler, TickOutcome};
pub use stats::TaskStats;

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
    /// Whether this lifecycle edge is allowed; self-transitions are errors.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Ready, Self::Running)
                | (
                    Self::Running,
                    Self::Ready | Self::Blocked | Self::Sleeping | Self::Zombie
                )
                | (Self::Blocked | Self::Sleeping, Self::Ready)
        )
    }

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
    /// All nonzero identifiers have been issued; identifiers are never reused.
    IdExhausted,
    /// The requested lifecycle edge is not legal.
    InvalidTransition,
    /// A different process is already running.
    AlreadyRunning,
    /// A time slice must contain at least one tick.
    InvalidQuantum,
    /// The operation requires a running process, but the scheduler is idle.
    NoCurrent,
    /// A requested sleep deadline is not strictly in the future.
    InvalidDeadline,
    /// Monotonic time or a relative deadline would overflow.
    ClockOverflow,
    /// Only exited, retained zombie processes may be reaped.
    NotZombie,
    /// The requested wait target is not a child of the running process.
    NotChild,
    /// An exited process cannot become the parent of a newly admitted task.
    InvalidParent,
}

/// Typed event identity. Event delivery and condition checks belong to the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitChannel(u64);

impl WaitChannel {
    /// Construct an event channel; zero is a valid event identity.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the numeric event identity.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Result of waiting for one specific child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitOutcome {
    /// The child had exited; its slot is now free and this is the final snapshot.
    Reaped(Process),
    /// The parent is blocked until the specified child exits.
    Blocked {
        /// Process selected after the parent blocked, or `None` for idle.
        next: Option<ProcessId>,
    },
}

/// Allocation-free metadata for one process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Process {
    id: ProcessId,
    state: ProcessState,
    wait: Option<WaitChannel>,
    deadline: Option<u64>,
    exit_code: Option<i32>,
    priority: Priority,
    missed_dispatches: u8,
    stats: TaskStats,
    parent: Option<ProcessId>,
    waiting_child: Option<ProcessId>,
}

impl Process {
    /// Live parent identity, or `None` for root/orphaned tasks.
    pub const fn parent(&self) -> Option<ProcessId> {
        self.parent
    }

    /// Child identity awaited by a blocked parent, if any.
    pub const fn waiting_for_child(&self) -> Option<ProcessId> {
        self.waiting_child
    }

    /// Copy the scheduling counters without exposing mutable task metadata.
    pub const fn stats(&self) -> TaskStats {
        self.stats
    }

    /// Configured base priority, before dispatch aging.
    pub const fn priority(&self) -> Priority {
        self.priority
    }

    /// Retained termination status; `None` means the process has not exited.
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Absolute monotonic wake tick while sleeping, if any.
    pub const fn wake_deadline(&self) -> Option<u64> {
        self.deadline
    }

    /// Event channel while blocked, if any.
    pub const fn wait_channel(&self) -> Option<WaitChannel> {
        self.wait
    }

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
    ids: PidAllocator,
}

impl<const N: usize> ProcessTable<N> {
    /// Construct an empty table.
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            ids: PidAllocator::new(),
        }
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
        let id = self.ids.allocate().ok_or(ProcessError::IdExhausted)?;
        self.slots[index] = Some(Process {
            id,
            state: ProcessState::Ready,
            wait: None,
            deadline: None,
            exit_code: None,
            priority: Priority::NORMAL,
            missed_dispatches: 0,
            stats: TaskStats::default(),
            parent: None,
            waiting_child: None,
        });
        Ok(id)
    }

    /// Apply one legal lifecycle edge while preserving at most one running task.
    pub fn transition(&mut self, id: ProcessId, next: ProcessState) -> Result<(), ProcessError> {
        let previous = self.get(id)?.state;
        if !previous.can_transition_to(next) {
            return Err(ProcessError::InvalidTransition);
        }
        if next == ProcessState::Running
            && self
                .slots
                .iter()
                .flatten()
                .any(|p| p.state == ProcessState::Running)
        {
            return Err(ProcessError::AlreadyRunning);
        }
        self.slots
            .iter_mut()
            .flatten()
            .find(|p| p.id == id)
            .unwrap()
            .state = next;
        Ok(())
    }

    fn get_mut(&mut self, id: ProcessId) -> Result<&mut Process, ProcessError> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|p| p.id == id)
            .ok_or(ProcessError::NotFound)
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

/// Monotonic identifier source that stops rather than wrapping or recycling.
pub struct PidAllocator {
    next: Option<u64>,
}

impl PidAllocator {
    /// Start allocating at identifier one.
    pub const fn new() -> Self {
        Self { next: Some(1) }
    }

    /// Start at an explicitly supplied nonzero identity, e.g. after a restore.
    pub const fn starting_at(first: ProcessId) -> Self {
        Self {
            next: Some(first.get()),
        }
    }

    /// Issue one unique identifier, or return `None` permanently after exhaustion.
    pub fn allocate(&mut self) -> Option<ProcessId> {
        let value = self.next?;
        self.next = value.checked_add(1);
        ProcessId::new(value)
    }
}

impl Default for PidAllocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
