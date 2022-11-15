use super::*;

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
