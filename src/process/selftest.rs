//! Bounded boot-time exercise of the scheduling model, not CPU execution.

use super::{
    InvariantError, ProcessError, ProcessState, ScheduleDecision, Scheduler, TickOutcome,
    WaitChannel,
};

/// Failure of the allocation-free scheduling boot diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelfTestError {
    /// A normally valid model operation was rejected.
    Operation(ProcessError),
    /// A structural invariant failed after a model operation.
    Invariant(InvariantError),
    /// An expected decision or snapshot differed, identified by a static step name.
    UnexpectedState(&'static str),
}

fn check(
    scheduler: &Scheduler<2>,
    expected: bool,
    step: &'static str,
) -> Result<(), SelfTestError> {
    scheduler
        .check_invariants()
        .map_err(SelfTestError::Invariant)?;
    if expected {
        Ok(())
    } else {
        Err(SelfTestError::UnexpectedState(step))
    }
}

/// Exercise dispatch, yield, sleep, event/deadline wakeup, exit, and reap locally.
///
/// Uses a fresh two-slot scheduler on the caller's stack, no heap or global state,
/// and checks structural invariants at each checkpoint. Safe to repeat before
/// installing the real scheduler; it consumes no real task IDs or clock ticks.
/// No CPU context, interrupts, hardware timers, or memory mappings are touched.
/// `Ok(())` proves model behavior only, not that tasks executed on hardware.
///
/// ```
/// assert_eq!(spark_os::process::self_test(), Ok(()));
/// ```
pub fn self_test() -> Result<(), SelfTestError> {
    let mut scheduler = Scheduler::<2>::with_quantum(2).map_err(SelfTestError::Operation)?;
    let idle = ScheduleDecision::Idle {
        next_deadline: None,
    };
    let decision = scheduler.schedule();
    check(&scheduler, decision == idle, "empty idle")?;

    let parent = scheduler.spawn().map_err(SelfTestError::Operation)?;
    check(&scheduler, scheduler.ready_len() == 1, "parent admission")?;
    let child = scheduler
        .spawn_child(parent)
        .map_err(SelfTestError::Operation)?;
    check(&scheduler, scheduler.ready_len() == 2, "child admission")?;
    let selected = scheduler.dispatch();
    check(&scheduler, selected == Some(parent), "dispatch")?;
    let selected = scheduler
        .yield_current()
        .map_err(SelfTestError::Operation)?;
    check(&scheduler, selected == Some(child), "yield")?;

    let selected = scheduler.sleep_for(2).map_err(SelfTestError::Operation)?;
    check(
        &scheduler,
        selected == Some(parent) && scheduler.next_wake_deadline() == Some(2),
        "sleep",
    )?;
    let channel = WaitChannel::new(1);
    let selected = scheduler
        .block_current(channel)
        .map_err(SelfTestError::Operation)?;
    check(&scheduler, selected.is_none(), "block")?;
    let decision = scheduler.schedule();
    check(
        &scheduler,
        decision
            == ScheduleDecision::Idle {
                next_deadline: Some(2),
            },
        "deadline idle",
    )?;

    let awakened = scheduler.wake_one(channel);
    check(
        &scheduler,
        awakened == Some(parent) && scheduler.current().is_none(),
        "event wake",
    )?;
    let duplicate = scheduler.wake_one(channel);
    check(
        &scheduler,
        duplicate.is_none() && scheduler.ready_len() == 1,
        "duplicate wake",
    )?;
    let selected = scheduler.dispatch();
    check(&scheduler, selected == Some(parent), "redispatch")?;
    let outcome = scheduler.tick().map_err(SelfTestError::Operation)?;
    check(
        &scheduler,
        outcome == TickOutcome::Continue(parent) && scheduler.ready_len() == 0,
        "first tick",
    )?;
    let outcome = scheduler.tick().map_err(SelfTestError::Operation)?;
    check(
        &scheduler,
        outcome == TickOutcome::QuantumExpired(parent)
            && scheduler.current() == Some(parent)
            && scheduler.ready_len() == 1
            && scheduler.next_wake_deadline().is_none(),
        "deadline wake and quantum expiry",
    )?;
    let selected = scheduler.reschedule();
    check(&scheduler, selected == Some(child), "explicit rotation")?;

    let selected = scheduler
        .exit_current(7)
        .map_err(SelfTestError::Operation)?;
    check(&scheduler, selected == Some(parent), "child exit")?;
    let child = scheduler.reap(child).map_err(SelfTestError::Operation)?;
    check(
        &scheduler,
        child.state() == ProcessState::Zombie
            && child.exit_code() == Some(7)
            && child.stats().sleeps == 1
            && child.stats().wakeups == 1
            && scheduler.processes().len() == 1,
        "child reap",
    )?;
    let selected = scheduler
        .exit_current(0)
        .map_err(SelfTestError::Operation)?;
    check(&scheduler, selected.is_none(), "parent exit")?;
    let parent = scheduler.reap(parent).map_err(SelfTestError::Operation)?;
    let decision = scheduler.schedule();
    check(
        &scheduler,
        parent.exit_code() == Some(0)
            && parent.stats().yields == 1
            && parent.stats().wakeups == 1
            && scheduler.processes().is_empty()
            && scheduler.ready_len() == 0
            && decision == idle,
        "final reap and idle",
    )
}
