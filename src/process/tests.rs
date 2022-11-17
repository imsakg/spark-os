use super::*;

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
