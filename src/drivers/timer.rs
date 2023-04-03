//! BCM system timer counter reads; this is not the ARM architectural timer.

use super::mmio::RegisterIo;

/// Timer sampling failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    /// High word changed during every permitted sample attempt.
    Unstable,
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
