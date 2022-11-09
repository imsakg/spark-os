use super::*;

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
