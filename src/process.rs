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

#[cfg(test)]
mod tests;
