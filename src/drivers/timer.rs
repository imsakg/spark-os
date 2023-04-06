//! BCM system timer counter reads; this is not the ARM architectural timer.

use super::mmio::RegisterIo;

/// Timer sampling failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    /// High word changed during every permitted sample attempt.
    Unstable,
    /// A relative delay must be within 1..=i32::MAX microseconds.
    InvalidDelay,
    /// Counter reached/passed the target before compare programming completed.
    DeadlineElapsed,
}

/// A low-word deadline, ordered only within a signed half-range of its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline(u32);
impl Deadline {
    /// Add a nonzero delay of at most `i32::MAX`, allowing low-word rollover.
    pub fn after(now: u32, delay: u32) -> Result<Self, TimerError> {
        if delay == 0 || delay > i32::MAX as u32 {
            return Err(TimerError::InvalidDelay);
        }
        Ok(Self(now.wrapping_add(delay)))
    }
    /// Return the programmed low-word compare target.
    pub fn target(self) -> u32 {
        self.0
    }
    /// Test signed wrapping distance; sample within a half-range of the target.
    pub fn reached(self, now: u32) -> bool {
        now.wrapping_sub(self.0) as i32 >= 0
    }
}

/// Compare channels normally available to the ARM; channels 0/2 are reserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AlarmChannel {
    /// System timer compare one.
    One = 1,
    /// System timer compare three.
    Three = 3,
}

/// BCM's free-running microsecond system timer register transport.
pub struct SystemTimer<T: RegisterIo> {
    io: T,
}
impl<T: RegisterIo> SystemTimer<T> {
    /// Bind a transport based at the timer CS register.
    pub fn new(io: T) -> Self {
        Self { io }
    }
    /// Recover transport ownership without changing timer state.
    pub fn release(self) -> T {
        self.io
    }
    /// Clear stale match state, program a relative compare, and detect races.
    ///
    /// Keep the controller IRQ masked until this succeeds. On `DeadlineElapsed`
    /// the compare remains programmed: keep it masked and handle the deadline
    /// in software or reprogram it. This does not install or enable an IRQ.
    pub fn arm_after(&mut self, channel: AlarmChannel, delay: u32) -> Result<Deadline, TimerError> {
        Deadline::after(0, delay)?;
        let target = Deadline::after(self.io.read32(4), delay)?;
        self.acknowledge(channel);
        self.io
            .write32(0x0c + (channel as usize) * 4, target.target());
        if target.reached(self.io.read32(4)) {
            return Err(TimerError::DeadlineElapsed);
        }
        Ok(target)
    }
    /// Read the channel's match-status bit; does not acknowledge it.
    pub fn alarm_pending(&mut self, channel: AlarmChannel) -> bool {
        self.io.read32(0) & (1 << channel as u32) != 0
    }
    /// Acknowledge only the selected channel using CS write-one-to-clear.
    pub fn acknowledge(&mut self, channel: AlarmChannel) {
        self.io.write32(0, 1 << channel as u32);
    }

    /// Sample CHI/CLO/CHI until both high words agree, bounded by `attempts`.
    /// Each attempt makes exactly three 32-bit reads; zero does no I/O.
    pub fn now(&mut self, attempts: usize) -> Result<u64, TimerError> {
        for _ in 0..attempts {
            let high = self.io.read32(8);
            let low = self.io.read32(4);
            if high == self.io.read32(8) {
                return Ok((u64::from(high) << 32) | u64::from(low));
            }
        }
        Err(TimerError::Unstable)
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;
    use crate::drivers::test_support::{Access::Read, FakeIo};
    #[test]
    fn stable_high_low_high_snapshot_and_rollover_retry() {
        let mut timer = SystemTimer::new(FakeIo::new([
            Read(8, 0),
            Read(4, u32::MAX),
            Read(8, 1),
            Read(8, 1),
            Read(4, 2),
            Read(8, 1),
        ]));
        assert_eq!(timer.now(2), Ok((1_u64 << 32) | 2));
        timer.release().done();
    }
    #[test]
    fn unstable_counter_is_bounded_and_zero_attempts_do_no_io() {
        let mut timer = SystemTimer::new(FakeIo::new([Read(8, 0), Read(4, 7), Read(8, 1)]));
        assert_eq!(timer.now(0), Err(TimerError::Unstable));
        assert_eq!(timer.now(1), Err(TimerError::Unstable));
        timer.release().done();
    }
}

#[cfg(test)]
mod alarm_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn deadlines_compare_safely_across_u32_wrap_with_half_range_limit() {
        let deadline = Deadline::after(u32::MAX - 2, 5).unwrap();
        assert_eq!(deadline.target(), 2);
        assert!(!deadline.reached(u32::MAX));
        assert!(!deadline.reached(1));
        assert!(deadline.reached(2));
        assert!(deadline.reached(3));
        assert_eq!(Deadline::after(0, 0), Err(TimerError::InvalidDelay));
        assert_eq!(Deadline::after(0, 1 << 31), Err(TimerError::InvalidDelay));
        assert!(Deadline::after(0, i32::MAX as u32).is_ok());
    }
    #[test]
    fn compare_one_programming_clears_only_owned_status_bit() {
        let mut timer = SystemTimer::new(FakeIo::new([
            Read(4, u32::MAX - 2),
            Write(0, 2),
            Write(0x10, 2),
            Read(4, u32::MAX - 1),
            Read(0, 0xb),
            Write(0, 2),
        ]));
        assert_eq!(timer.arm_after(AlarmChannel::One, 5).unwrap().target(), 2);
        assert!(timer.alarm_pending(AlarmChannel::One));
        timer.acknowledge(AlarmChannel::One);
        timer.release().done();
    }
    #[test]
    fn compare_three_detects_a_deadline_missed_during_programming() {
        let mut timer = SystemTimer::new(FakeIo::new([
            Read(4, 100),
            Write(0, 8),
            Write(0x18, 105),
            Read(4, 106),
        ]));
        assert_eq!(
            timer.arm_after(AlarmChannel::Three, 0),
            Err(TimerError::InvalidDelay)
        );
        assert_eq!(
            timer.arm_after(AlarmChannel::Three, 5),
            Err(TimerError::DeadlineElapsed)
        );
        timer.release().done();
    }
}
