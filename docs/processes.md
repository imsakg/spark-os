# Processes and cooperative scheduling

The `spark_os::process` library is an allocation-free, single-core **decision
model**, available to the bare-metal binary and exercised by dependency-free host
tests. It owns fixed-size process metadata and a ready queue. It does not execute
task entry points, switch CPU registers, install exception handlers, or implement
preemption. A process marked `Running` is the scheduler's selection, not evidence
that its code ran on hardware.

## Boot integration

Public entry point:

```text
spark_os::process::self_test() -> Result<(), spark_os::process::SelfTestError>
```

Call after stack/BSS initialization, and after console initialization if printing
a success marker. This can run before creating the real scheduler:

```rust
match spark_os::process::self_test() {
    Ok(()) => println!("[process] model selftest passed"),
    Err(error) => panic!("process model selftest failed: {:?}", error),
}
```

The helper uses a fresh two-slot scheduler on the caller's stack. It checks empty
idle, parent/child admission, dispatch, voluntary yield, deadline sleep,
event-blocked idle, event wakeup without duplicate admission, exact deadline
wakeup, quantum expiry without an implicit rotation, explicit rescheduling,
exit-status retention, reaping, and the final empty idle state. Every checkpoint
runs `check_invariants()`. There is no heap, global state, real tick source,
hardware timer, interrupt manipulation, register capture, or task execution.
Repeated calls do not consume identities or ticks from another scheduler.

`SelfTestError::Operation` retains a rejected `ProcessError`;
`SelfTestError::Invariant` identifies a broken structural invariant;
`SelfTestError::UnexpectedState` carries a static checkpoint label. A success
marker means **the model selftest passed**, not that CPU context switching works.
The helper returns errors rather than printing; boot decides whether to stop.
This branch deliberately does not edit `src/main.rs` or existing boot markers.

## Ownership and capacity

- `Scheduler::<N>::new()` is an empty, constant-initializable scheduler with
  round-robin policy, a four-tick quantum, and clock zero. `with_quantum` rejects
  zero; `with_clock` additionally accepts a monotonic epoch.
- `N` bounds all retained tasks, including zombies. The table and ready ring are
  inline arrays; neither uses `alloc`, a heap, nor external crates. Zero capacity
  is valid and always idle. Size `N` for the available kernel stack/static memory.
- `spawn()` admits only metadata, initially `Ready`; it does not create a stack,
  address space, executable image, thread, or hardware context. `spawn_child`
  requires a live parent and inherits its base priority.
- Identities start at one and increase monotonically within one scheduler. Zero
  is reserved; reaping releases a slot but never reuses the old PID. Exhaustion
  returns `IdExhausted`, rather than wrapping. A full-table rejection consumes no
  PID. IDs from independent scheduler instances are not globally unique.
- All mutation requires exclusive ownership. There are no built-in locks or
  interrupt guards. A kernel owner must serialize task calls and IRQ event/tick
  delivery; do not create aliased mutable references to a global scheduler.
- `ProcessTable` and `RunQueue` are also available independently. The table's
  low-level `transition` checks legal state edges and exclusive running state
  only; it does not fill wait/deadline/exit metadata. Use `Scheduler` operations
  for a complete scheduling model. `processes()` exposes only shared access.

## Lifecycle and dispatch

Allowed state edges are:

```text
Ready -> Running
Running -> Ready | Blocked | Sleeping | Zombie
Blocked | Sleeping -> Ready
```

Self-transitions and every edge out of `Zombie` are rejected. `ProcessError`
distinguishes missing/stale IDs, table/PID exhaustion, invalid transitions,
missing current task, invalid quantum/deadline, overflow, and child ownership.
Rejected `Result` operations preserve the scheduler's state.

- `dispatch()` chooses a task only while no current task exists. Repeated calls
  return the existing selection and do not replenish its remaining slice.
- `schedule()` is the same selection expressed as `ScheduleDecision::Run(id)` or
  `Idle { next_deadline }`. Idle consumes no PID or process slot.
- `yield_current()` donates the unused slice and rejoins the ready tail. It
  immediately dispatches another eligible task, or the same task if alone.
  Yielding while idle returns `NoCurrent` without dispatching queued work.
- `reschedule()` explicitly returns the running task to the ready tail and
  selects again; while idle, it may dispatch queued work. Despite its name it
  does not switch CPU execution. `dispatch()`/`schedule()` alone do not rotate an
  expired running task.
- The ready queue contains each `Ready` task exactly once, never the current,
  blocked, sleeping, or zombie task. `ready_ids()` gives FIFO admission order,
  not the order that a priority policy will select.

## Time slices, sleeps, and idle

`tick()` advances a checked monotonic counter by one. It charges running/ready
statistics based on state at tick entry, admits due sleepers in ascending
`(deadline, PID)` order, then returns `Idle`, `Continue(id)`, or
`QuantumExpired(id)`. Expiry does not alter `current()`. If the owner does not
reschedule, further ticks keep reporting expiry and counting runtime. This is a
request for a safe scheduling point, **not interrupt-driven preemption**.

```rust
use spark_os::process::{ProcessError, Scheduler, TickOutcome};
# fn main() -> Result<(), ProcessError> {
let mut scheduler = Scheduler::<2>::with_quantum(1)?;
let first = scheduler.spawn()?;
let second = scheduler.spawn()?;
assert_eq!(scheduler.dispatch(), Some(first));
let needs_rotation = matches!(scheduler.tick()?, TickOutcome::QuantumExpired(_));
assert_eq!(scheduler.current(), Some(first)); // Still only a metadata selection.
if needs_rotation {
    // At an owner-controlled safe point, not an unimplemented IRQ switch.
    assert_eq!(scheduler.reschedule(), Some(second));
}
# Ok(())
# }
```

`sleep_until(deadline)` requires a strictly future tick; `sleep_for(ticks)`
requires a positive, non-overflowing duration. Both stop the current task and
immediately select another, or idle. Due sleepers become ready only on a
successful tick, not through `wake(id)`. Relative-deadline/clock overflow returns
`ClockOverflow` before changing counters, budgets, or wakeups. Time never wraps.

A tick received with no current task returns `Idle` and counts an idle tick even
if that tick makes a sleeper ready. Call `schedule()` afterward to select it.
`next_wake_deadline()` and idle decisions report the earliest sleeping deadline,
or `None` for an empty/event-only idle state. They do not program a timer or
execute `WFI`. The kernel must arrange both deadline and external-event wakeups
and close the condition-check/interrupt-unmask/CPU-sleep race. A tickless elapsed-
time API is not implemented: one `tick()` always represents one model tick.

## Event waits and child collection

`WaitChannel` is a typed event key; zero is valid. `block_current(channel)`
records one wait reason and dispatches a successor. `wake(id)` affects only an
event-blocked task; `wake_one(channel)` selects the lowest-PID matching waiter;
`wake_all(channel)` admits matches in ascending PID order and reports the count.
These wake operations make tasks ready without implicit dispatch/preemption.
Repeated delivery cannot duplicate an admission. Event delivery is not latched:
the caller must atomically coordinate its actual condition check and blocking
with event producers, normally under the same ownership/IRQ-exclusion protocol.

`wait_child(child)` requires the running parent. It either reaps an already
exited child (`WaitOutcome::Reaped`) or blocks for that particular child
(`WaitOutcome::Blocked { next }`). An unrelated exit or event wake cannot satisfy
that wait. Child exit wakes the waiting parent; call `wait_child` again after the
parent is selected to collect the snapshot. Parent exit orphans its children;
there is no implicit init process or automatic child adoption.

```rust
use spark_os::process::{ProcessError, ScheduleDecision, Scheduler, WaitOutcome};
# fn main() -> Result<(), ProcessError> {
let mut scheduler = Scheduler::<2>::new();
let parent = scheduler.spawn()?;
let child = scheduler.spawn_child(parent)?;
assert_eq!(scheduler.schedule(), ScheduleDecision::Run(parent));
assert_eq!(scheduler.wait_child(child)?, WaitOutcome::Blocked { next: Some(child) });
assert_eq!(scheduler.exit_current(7)?, Some(parent));
match scheduler.wait_child(child)? {
    WaitOutcome::Reaped(snapshot) => assert_eq!(snapshot.exit_code(), Some(7)),
    WaitOutcome::Blocked { .. } => panic!("child has already exited"),
}
assert_eq!(scheduler.exit_current(0)?, None);
assert_eq!(scheduler.reap(parent)?.exit_code(), Some(0));
assert!(scheduler.processes().is_empty());
assert_eq!(scheduler.check_invariants(), Ok(()));
# Ok(())
# }
```

`exit_current(code)` retains a zombie's exit code, context snapshot, and counters
and selects another task. Zombies still occupy slots. `reap(id)` is privileged
owner cleanup for zombies only; `wait_child` adds parent-ownership checking.
Reaping returns the complete `Process` snapshot. Later use of that stale PID is
`NotFound`; a live task is `NotZombie`. There is no automatic zombie collection.

## Policy and observability

Round-robin ignores priorities and preserves FIFO rotation. Optional
`SchedulingPolicy::PriorityAging` selects the lowest effective priority, breaking
ties by FIFO admission. `Priority::new` accepts `0..=7`, where zero is most
urgent; `Priority::NORMAL` is four. Every `AGING_INTERVAL` (two) missed dispatches
promotes a waiting task by one level, down to zero, without changing its base
priority. Selected tasks reset their aging credit. `set_policy` resets all aging
credits; `set_priority` changes the base priority. Neither interrupts the current
task. Fairness depends on continued scheduling points and bounded task capacity;
this is not a real-time latency guarantee or protection against a task that
never yields.

`Process::stats()` returns saturating counters for runtime/ready ticks, actual
dispatches, voluntary yields, blocks, sleeps, and wakeups. Counters survive exit
and reap. `idle_ticks()` is separate kernel-idle accounting. They measure model
operations, not CPU cycles or wall-clock durations. `ProcessTable::iter()`
includes retained zombies in storage order.

`check_invariants() -> Result<(), InvariantError>` is an allocation-free O(N²)
diagnostic for identity monotonicity/uniqueness, ready membership, unique current,
slice bounds, lifecycle wait/exit metadata, and live acyclic parent/child links.
It is intended for boot/model tests and controlled diagnostics, not an IRQ fast
path or recovery from arbitrary memory corruption. The host suite also compares
the queue with a reference model, mixes scheduler operations at zero/single/
multiple capacities, checks error atomicity, and exercises starvation, counter
saturation, PID exhaustion, and clock overflow.

## Context data is not a context switch

`SavedContext::for_kernel(entry, stack_top)` validates nonzero four-byte-aligned
PC and nonzero sixteen-byte-aligned SP. `set_context` attaches that caller-
supplied data to a live task. A `repr(C, align(16))` frame stores x19 through x30
in word offsets 0 through 11, then SP and PC in offsets 12 and 13. Register
get/set validates that only x19 through x30 are addressed.

This does not validate executable/mapped addresses or allocate stack memory.
Initial x30 is zero, not a safe task-return trampoline. Caller-saved registers,
PSTATE, SIMD/FP, EL0 exception frames, address-space management, save/restore
assembly, and multicore ownership are not implemented. A future hardware switch
must supply those contracts, stack lifetime/alignment, and interrupt-safe handoff
before claiming real process execution. No unsafe/assembly switch stub is hidden
behind this API.

## Reproduce the gates

Run from the repository root with the pinned `nightly-2022-10-13` toolchain
(Rust 1.66 nightly). These paths bypass Cargo registry resolution; the process
library needs only `core`:

```sh
sh tools/host-test.sh
rustfmt --edition 2021 --check src/process.rs
rustc --edition=2021 --crate-name spark_os --crate-type rlib src/lib.rs \
  -D warnings -D missing_docs --out-dir target/host
rustdoc --edition=2021 --test src/lib.rs \
  --extern spark_os=target/host/libspark_os.rlib -D warnings
rustdoc --edition=2021 --test docs/processes.md \
  --extern spark_os=target/host/libspark_os.rlib -D warnings
mkdir -p target/process-aarch64
rustc --edition=2021 --crate-name spark_os --crate-type rlib src/lib.rs \
  --target aarch64-unknown-none-softfloat \
  -D warnings -D missing_docs --out-dir target/process-aarch64
```

The host script already compiles tests with `-D warnings`. A successful AArch64
library compile proves the `no_std` API builds for that target; it is not a boot,
IRQ, hardware timer, or task-switch execution test. Boot wiring/QEMU integration
belongs to the kernel owner and is intentionally outside this process branch.
