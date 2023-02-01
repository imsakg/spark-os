//! Deterministic reference-model and boundary checks, without external crates.

use super::*;
use std::{collections::VecDeque, vec::Vec};

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    *seed >> 32
}

fn queue_model<const N: usize>() {
    let mut actual = RunQueue::<N>::new();
    let mut expected = VecDeque::new();
    let mut seed = 37;
    for _ in 0..2048 {
        let id = ProcessId::new(next(&mut seed) % 7 + 1).unwrap();
        match next(&mut seed) % 3 {
            0 => {
                let result = if expected.contains(&id) {
                    Err(QueueError::Duplicate)
                } else if expected.len() == N {
                    Err(QueueError::Full)
                } else {
                    expected.push_back(id);
                    Ok(())
                };
                assert_eq!(actual.enqueue(id), result);
            }
            1 => assert_eq!(actual.pop(), expected.pop_front()),
            _ => {
                let position = expected.iter().position(|entry| *entry == id);
                if let Some(position) = position {
                    expected.remove(position);
                }
                assert_eq!(actual.remove(id), position.is_some());
            }
        }
        assert_eq!(
            actual.iter().collect::<Vec<_>>(),
            expected.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(actual.len(), expected.len());
    }
}

#[test]
fn run_queue_matches_reference_model_at_zero_single_and_wrapping_capacities() {
    queue_model::<0>();
    queue_model::<1>();
    queue_model::<3>();
}

#[derive(Debug, Eq, PartialEq)]
struct Snapshot {
    tasks: Vec<Process>,
    current: Option<ProcessId>,
    ready: Vec<ProcessId>,
    now: u64,
    remaining: u32,
    idle: u64,
}

fn snapshot<const N: usize>(scheduler: &Scheduler<N>) -> Snapshot {
    Snapshot {
        tasks: scheduler.processes().iter().copied().collect(),
        current: scheduler.current(),
        ready: scheduler.ready_ids().collect(),
        now: scheduler.now(),
        remaining: scheduler.remaining_quantum(),
        idle: scheduler.idle_ticks(),
    }
}

fn scheduler_model<const N: usize>() {
    let mut scheduler = Scheduler::<N>::with_quantum(3).unwrap();
    let context = SavedContext::for_kernel(0x1000, 0x8000).unwrap();
    let mut seed = 123;
    let mut highest_id = 0;
    for step in 0..4096 {
        let before = snapshot(&scheduler);
        let number = next(&mut seed);
        let id = before
            .tasks
            .get(number as usize % before.tasks.len().max(1))
            .map(|p| p.id())
            .unwrap_or(ProcessId::new(10000).unwrap());
        let channel = WaitChannel::new(number % 3);
        let operation = next(&mut seed) % 17;
        let result: Result<(), ProcessError> = match operation {
            0 | 14 => {
                let result = if operation == 0 {
                    scheduler.spawn()
                } else {
                    scheduler.spawn_child(id)
                };
                result.map(|new_id| {
                    assert_eq!(new_id.get(), highest_id + 1);
                    highest_id = new_id.get();
                })
            }
            1 => {
                scheduler.dispatch();
                Ok(())
            }
            2 => {
                scheduler.reschedule();
                Ok(())
            }
            3 => scheduler.yield_current().map(|_| ()),
            4 => scheduler.block_current(channel).map(|_| ()),
            5 => scheduler.wake(id),
            6 => {
                scheduler.wake_one(channel);
                Ok(())
            }
            7 => {
                scheduler.wake_all(channel);
                Ok(())
            }
            8 => scheduler.sleep_for(number % 5).map(|_| ()),
            9 => scheduler.tick().map(|_| ()),
            10 => scheduler.exit_current(number as i32).map(|_| ()),
            11 => scheduler.reap(id).map(|_| ()),
            12 => scheduler.set_priority(id, Priority::new((number % 8) as u8).unwrap()),
            13 => {
                scheduler.set_policy(if number % 2 == 0 {
                    SchedulingPolicy::RoundRobin
                } else {
                    SchedulingPolicy::PriorityAging
                });
                Ok(())
            }
            15 => scheduler.wait_child(id).map(|_| ()),
            _ => scheduler.set_context(id, context),
        };
        assert_eq!(
            scheduler.check_invariants(),
            Ok(()),
            "step {}, op {}",
            step,
            operation
        );
        if result.is_err() {
            assert_eq!(
                snapshot(&scheduler),
                before,
                "rejected operation {} at step {} mutated state",
                operation,
                step
            );
        }
        assert!(scheduler.processes().len() <= N);
    }
}

#[test]
fn mixed_scheduler_sequences_preserve_invariants_and_error_atomicity() {
    scheduler_model::<0>();
    scheduler_model::<1>();
    scheduler_model::<5>();
}

#[test]
fn priority_aging_serves_every_base_priority_with_urgent_competition() {
    for base in 0..=7 {
        let mut scheduler = Scheduler::<8>::new();
        scheduler.set_policy(SchedulingPolicy::PriorityAging);
        let watched = scheduler
            .spawn_with_priority(Priority::new(base).unwrap())
            .unwrap();
        for _ in 0..7 {
            scheduler
                .spawn_with_priority(Priority::new(0).unwrap())
                .unwrap();
        }
        let mut seen = false;
        for _ in 0..23 {
            if scheduler.reschedule() == Some(watched) {
                seen = true;
                break;
            }
        }
        assert!(seen, "base priority {} starved", base);
        assert_eq!(scheduler.check_invariants(), Ok(()));
    }
}

#[test]
fn unrelated_child_exit_does_not_satisfy_a_specific_wait() {
    let mut scheduler = Scheduler::<3>::new();
    let parent = scheduler.spawn().unwrap();
    let other = scheduler.spawn_child(parent).unwrap();
    let wanted = scheduler.spawn_child(parent).unwrap();
    scheduler.dispatch();
    assert_eq!(
        scheduler.wait_child(wanted),
        Ok(WaitOutcome::Blocked { next: Some(other) })
    );
    scheduler.exit_current(2).unwrap();
    assert_eq!(scheduler.current(), Some(wanted));
    assert_eq!(
        scheduler.processes().get(parent).unwrap().state(),
        ProcessState::Blocked
    );
    scheduler.exit_current(3).unwrap();
    assert_eq!(scheduler.current(), Some(parent));
    assert!(matches!(
        scheduler.wait_child(wanted),
        Ok(WaitOutcome::Reaped(_))
    ));
    assert!(matches!(
        scheduler.wait_child(other),
        Ok(WaitOutcome::Reaped(_))
    ));
    assert_eq!(scheduler.check_invariants(), Ok(()));
}

#[test]
fn overflow_does_not_charge_task_statistics_or_wake_sleepers() {
    let mut scheduler = Scheduler::<2>::with_clock(1, u64::MAX - 1).unwrap();
    scheduler.spawn().unwrap();
    scheduler.spawn().unwrap();
    scheduler.dispatch();
    scheduler.sleep_until(u64::MAX).unwrap();
    scheduler.tick().unwrap();
    let before = snapshot(&scheduler);
    assert_eq!(scheduler.tick(), Err(ProcessError::ClockOverflow));
    assert_eq!(snapshot(&scheduler), before);
    assert_eq!(scheduler.check_invariants(), Ok(()));
}
