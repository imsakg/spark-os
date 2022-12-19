use super::*;

#[test]
fn sleepers_wake_at_exact_deadlines_in_stable_order() {
    let mut scheduler = Scheduler::<2>::new();
    let a = scheduler.spawn().unwrap();
    let b = scheduler.spawn().unwrap();
    scheduler.dispatch();
    assert_eq!(scheduler.sleep_for(0), Err(ProcessError::InvalidDeadline));
    assert_eq!(scheduler.current(), Some(a));
    assert_eq!(scheduler.sleep_until(2), Ok(Some(b)));
    assert_eq!(
        scheduler.processes().get(a).unwrap().wake_deadline(),
        Some(2)
    );
    assert_eq!(scheduler.wake(a), Err(ProcessError::InvalidTransition));
    assert_eq!(scheduler.sleep_for(2), Ok(None));
    assert_eq!(scheduler.tick(), Ok(TickOutcome::Idle));
    assert_eq!(scheduler.now(), 1);
    assert_eq!(scheduler.ready_len(), 0);
    assert_eq!(scheduler.tick(), Ok(TickOutcome::Idle));
    assert_eq!(scheduler.ready_len(), 2);
    assert_eq!(scheduler.processes().get(a).unwrap().wake_deadline(), None);
    assert_eq!(scheduler.dispatch(), Some(a));
    assert_eq!(scheduler.reschedule(), Some(b));
}

#[test]
fn clock_and_deadline_overflow_are_atomic_errors() {
    let mut scheduler = Scheduler::<1>::with_clock(2, u64::MAX).unwrap();
    let id = scheduler.spawn().unwrap();
    scheduler.dispatch();
    assert_eq!(scheduler.tick(), Err(ProcessError::ClockOverflow));
    assert_eq!(scheduler.sleep_for(1), Err(ProcessError::ClockOverflow));
    assert_eq!(scheduler.remaining_quantum(), 2);
    assert_eq!(scheduler.current(), Some(id));
    assert_eq!(scheduler.now(), u64::MAX);
    assert_eq!(
        scheduler.sleep_until(u64::MAX),
        Err(ProcessError::InvalidDeadline)
    );
}

#[test]
fn wakeups_match_channels_and_never_duplicate_ready_admissions() {
    let mut scheduler = Scheduler::<3>::new();
    let [a, b, c] = [
        scheduler.spawn().unwrap(),
        scheduler.spawn().unwrap(),
        scheduler.spawn().unwrap(),
    ];
    let event = WaitChannel::new(1);
    let other = WaitChannel::new(2);
    scheduler.dispatch();
    scheduler.block_current(event).unwrap();
    scheduler.block_current(other).unwrap();
    scheduler.block_current(event).unwrap();
    assert_eq!(scheduler.wake_one(event), Some(a));
    assert_eq!(scheduler.wake(a), Err(ProcessError::InvalidTransition));
    assert_eq!(scheduler.processes().get(a).unwrap().wait_channel(), None);
    assert_eq!(scheduler.wake_all(event), 1);
    assert_eq!(scheduler.wake_all(event), 0);
    assert_eq!(scheduler.wake_one(other), Some(b));
    assert_eq!(scheduler.ready_len(), 3);
    assert_eq!(scheduler.current(), None);
    assert_eq!(scheduler.dispatch(), Some(a));
    assert_eq!(scheduler.reschedule(), Some(c));
    assert_eq!(scheduler.reschedule(), Some(b));
    assert_eq!(
        scheduler.wake(ProcessId::new(99).unwrap()),
        Err(ProcessError::NotFound)
    );
}

#[test]
fn blocking_removes_current_until_an_explicit_wakeup() {
    let mut scheduler = Scheduler::<2>::new();
    let channel = WaitChannel::new(12);
    assert_eq!(channel.get(), 12);
    assert_eq!(
        scheduler.block_current(channel),
        Err(ProcessError::NoCurrent)
    );
    let a = scheduler.spawn().unwrap();
    let b = scheduler.spawn().unwrap();
    scheduler.dispatch();
    assert_eq!(scheduler.block_current(channel), Ok(Some(b)));
    let blocked = scheduler.processes().get(a).unwrap();
    assert_eq!(blocked.state(), ProcessState::Blocked);
    assert_eq!(blocked.wait_channel(), Some(channel));
    assert_eq!(scheduler.block_current(channel), Ok(None));
    assert_eq!(scheduler.remaining_quantum(), 0);
    assert_eq!(scheduler.reschedule(), None);
    assert_eq!(scheduler.ready_len(), 0);
    assert_eq!(scheduler.processes().len(), 2);
}

#[test]
fn voluntary_yield_donates_slice_and_preserves_fifo_fairness() {
    let mut scheduler = Scheduler::<2>::with_quantum(5).unwrap();
    assert_eq!(scheduler.yield_current(), Err(ProcessError::NoCurrent));
    let a = scheduler.spawn().unwrap();
    let b = scheduler.spawn().unwrap();
    scheduler.dispatch();
    scheduler.tick().unwrap();
    assert_eq!(scheduler.remaining_quantum(), 4);
    assert_eq!(scheduler.yield_current(), Ok(Some(b)));
    assert_eq!(scheduler.remaining_quantum(), 5);
    assert_eq!(
        scheduler.processes().get(a).unwrap().state(),
        ProcessState::Ready
    );
    assert_eq!(scheduler.yield_current(), Ok(Some(a)));
    let mut one = Scheduler::<1>::with_quantum(3).unwrap();
    let id = one.spawn().unwrap();
    one.dispatch();
    one.tick().unwrap();
    assert_eq!(one.yield_current(), Ok(Some(id)));
    assert_eq!(one.remaining_quantum(), 3);
    assert_eq!(one.ready_len(), 0);
}

#[test]
fn quantum_ticks_request_rescheduling_without_claiming_preemption() {
    assert!(matches!(
        Scheduler::<1>::with_quantum(0),
        Err(ProcessError::InvalidQuantum)
    ));
    let mut scheduler = Scheduler::<2>::with_quantum(2).unwrap();
    assert_eq!(scheduler.tick(), Ok(TickOutcome::Idle));
    let a = scheduler.spawn().unwrap();
    let b = scheduler.spawn().unwrap();
    scheduler.dispatch();
    assert_eq!(scheduler.remaining_quantum(), 2);
    assert_eq!(scheduler.tick(), Ok(TickOutcome::Continue(a)));
    assert_eq!(scheduler.remaining_quantum(), 1);
    assert_eq!(scheduler.tick(), Ok(TickOutcome::QuantumExpired(a)));
    assert_eq!(scheduler.tick(), Ok(TickOutcome::QuantumExpired(a)));
    assert_eq!(scheduler.current(), Some(a));
    assert_eq!(scheduler.reschedule(), Some(b));
    assert_eq!(scheduler.remaining_quantum(), 2);
    assert_eq!(scheduler.tick(), Ok(TickOutcome::Continue(b)));
}

#[test]
fn scheduler_admits_and_rotates_in_round_robin_order() {
    let mut scheduler = Scheduler::<3>::new();
    assert_eq!(scheduler.dispatch(), None);
    let a = scheduler.spawn().unwrap();
    let b = scheduler.spawn().unwrap();
    let c = scheduler.spawn().unwrap();
    assert_eq!(scheduler.spawn(), Err(ProcessError::TableFull));
    assert_eq!(scheduler.ready_len(), 3);
    assert_eq!(scheduler.dispatch(), Some(a));
    assert_eq!(scheduler.dispatch(), Some(a));
    assert_eq!(scheduler.current(), Some(a));
    assert_eq!(
        scheduler.processes().get(a).unwrap().state(),
        ProcessState::Running
    );
    for expected in [b, c, a, b, c, a] {
        assert_eq!(scheduler.reschedule(), Some(expected));
        assert_eq!(scheduler.ready_len(), 2);
    }
    let mut one = Scheduler::<1>::new();
    let id = one.spawn().unwrap();
    assert_eq!(one.reschedule(), Some(id));
    assert_eq!(one.reschedule(), Some(id));
}

#[test]
fn run_queue_is_fifo_duplicate_free_and_wraps_without_losing_entries() {
    let [a, b, c, d] = [1, 2, 3, 4].map(|n| ProcessId::new(n).unwrap());
    let mut queue = RunQueue::<3>::new();
    assert_eq!(queue.pop(), None);
    queue.enqueue(a).unwrap();
    queue.enqueue(b).unwrap();
    queue.enqueue(c).unwrap();
    assert_eq!(queue.enqueue(a), Err(QueueError::Duplicate));
    assert_eq!(queue.enqueue(d), Err(QueueError::Full));
    assert_eq!(queue.pop(), Some(a));
    queue.enqueue(d).unwrap();
    assert!(queue.remove(c));
    assert!(!queue.remove(a));
    assert_eq!(queue.iter().collect::<std::vec::Vec<_>>(), [b, d]);
    assert_eq!(queue.pop(), Some(b));
    assert_eq!(queue.pop(), Some(d));
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert_eq!(RunQueue::<0>::new().enqueue(a), Err(QueueError::Full));
    assert!(!RunQueue::<0>::new().remove(a));
}

#[test]
fn table_transitions_preserve_single_running_process() {
    let mut table = ProcessTable::<2>::new();
    let a = table.spawn().unwrap();
    let b = table.spawn().unwrap();
    table.transition(a, ProcessState::Running).unwrap();
    assert_eq!(
        table.transition(b, ProcessState::Running),
        Err(ProcessError::AlreadyRunning)
    );
    assert_eq!(table.get(b).unwrap().state(), ProcessState::Ready);
    table.transition(a, ProcessState::Blocked).unwrap();
    table.transition(b, ProcessState::Running).unwrap();
    table.transition(a, ProcessState::Ready).unwrap();
    table.transition(b, ProcessState::Zombie).unwrap();
    assert_eq!(
        table.transition(b, ProcessState::Ready),
        Err(ProcessError::InvalidTransition)
    );
    assert_eq!(
        table.transition(a, ProcessState::Sleeping),
        Err(ProcessError::InvalidTransition)
    );
    assert_eq!(
        table.transition(a, ProcessState::Ready),
        Err(ProcessError::InvalidTransition)
    );
    assert_eq!(
        table.transition(ProcessId::new(77).unwrap(), ProcessState::Ready),
        Err(ProcessError::NotFound)
    );
}

#[test]
fn lifecycle_transition_matrix_is_explicit() {
    use ProcessState::*;
    let states = [Ready, Running, Blocked, Sleeping, Zombie];
    for from in states {
        for to in states {
            let expected = matches!(
                (from, to),
                (Ready, Running)
                    | (Running, Ready)
                    | (Running, Blocked)
                    | (Running, Sleeping)
                    | (Running, Zombie)
                    | (Blocked, Ready)
                    | (Sleeping, Ready)
            );
            assert_eq!(
                from.can_transition_to(to),
                expected,
                "{:?} -> {:?}",
                from,
                to
            );
        }
    }
}

#[test]
fn pid_allocator_is_monotonic_and_never_wraps_to_zero() {
    let mut ids = PidAllocator::new();
    assert_eq!(ids.allocate().unwrap().get(), 1);
    assert_eq!(ids.allocate().unwrap().get(), 2);
    let mut edge = PidAllocator::starting_at(ProcessId::new(u64::MAX).unwrap());
    assert_eq!(edge.allocate().unwrap().get(), u64::MAX);
    assert_eq!(edge.allocate(), None);
    assert_eq!(edge.allocate(), None);
}

#[test]
fn failed_table_allocation_does_not_consume_an_identifier() {
    let mut table = ProcessTable::<1>::new();
    let first = table.spawn().unwrap();
    assert_eq!(table.spawn(), Err(ProcessError::TableFull));
    assert_eq!(table.ids.allocate().unwrap().get(), first.get() + 1);
    table.slots[0] = None;
    table.ids = PidAllocator { next: None };
    assert_eq!(table.spawn(), Err(ProcessError::IdExhausted));
    assert!(table.is_empty());
}

#[test]
fn table_allocates_bounded_slots_and_checks_unknown_ids() {
    let mut table = ProcessTable::<2>::new();
    assert!(table.is_empty());
    let first = table.spawn().unwrap();
    let second = table.spawn().unwrap();
    assert_ne!(first, second);
    assert_eq!(table.len(), 2);
    assert_eq!(table.capacity(), 2);
    assert_eq!(table.get(first).unwrap().id(), first);
    assert_eq!(table.get(second).unwrap().state(), ProcessState::Ready);
    assert_eq!(table.spawn(), Err(ProcessError::TableFull));
    assert_eq!(
        table.get(ProcessId::new(99).unwrap()),
        Err(ProcessError::NotFound)
    );
}

#[test]
fn zero_capacity_table_rejects_allocation() {
    assert_eq!(
        ProcessTable::<0>::new().spawn(),
        Err(ProcessError::TableFull)
    );
}

#[test]
fn process_ids_reserve_zero_and_compare_by_value() {
    assert_eq!(ProcessId::new(0), None);
    assert_eq!(ProcessId::new(42).unwrap().get(), 42);
    assert!(ProcessId::new(1) < ProcessId::new(2));
}

#[test]
fn process_states_classify_schedulable_and_terminal_tasks() {
    assert!(ProcessState::Ready.is_runnable());
    assert!(ProcessState::Running.is_runnable());
    for state in [
        ProcessState::Blocked,
        ProcessState::Sleeping,
        ProcessState::Zombie,
    ] {
        assert!(!state.is_runnable());
    }
    assert!(ProcessState::Zombie.is_terminal());
    assert!(!ProcessState::Ready.is_terminal());
}
