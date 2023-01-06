//! Checked priorities and dispatch-based starvation avoidance.

/// Every two missed dispatches promote a waiting task by one priority level.
pub const AGING_INTERVAL: u8 = 2;

/// Priority in the inclusive range 0..=7; zero is most urgent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Priority(u8);

impl Priority {
    /// Default priority assigned to new processes.
    pub const NORMAL: Self = Self(4);

    /// Validate an external numeric priority.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 7 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the configured numeric priority.
    pub const fn get(self) -> u8 {
        self.0
    }

    pub(super) fn aged(self, missed: u8) -> u8 {
        self.0.saturating_sub(missed / AGING_INTERVAL)
    }
}

/// Selection policy used only at explicit dispatch points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulingPolicy {
    /// FIFO rotation, independent of configured priority.
    RoundRobin,
    /// Lowest effective priority first, promoting missed dispatches; FIFO ties.
    PriorityAging,
}
