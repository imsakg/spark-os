//! Snapshot counters for observable scheduling behavior.

/// Saturating, per-process counters retained across exit and reap.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskStats {
    /// Ticks spent running, including ticks after slice expiry before reschedule.
    pub runtime_ticks: u64,
    /// Ticks spent ready at the start of the tick, before deadline wakeups.
    pub ready_ticks: u64,
    /// Actual ready-to-running selections, not repeated dispatch queries.
    pub dispatches: u64,
    /// Explicit voluntary yields, excluding forced policy rotations.
    pub yields: u64,
    /// Event/child waits that stopped the process.
    pub blocks: u64,
    /// Deadline sleeps that stopped the process.
    pub sleeps: u64,
    /// Blocked/sleeping-to-ready transitions.
    pub wakeups: u64,
}
