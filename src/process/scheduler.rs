//! Exclusive-owner single-core scheduling decisions, not hardware switching.

use super::{
    InvariantError, Priority, Process, ProcessError, ProcessId, ProcessState, ProcessTable,
    RunQueue, SavedContext, SchedulingPolicy, WaitChannel, WaitOutcome,
};

/// A tick reports policy only; it never switches CPU context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickOutcome {
    /// No process was running during this tick.
    Idle,
    /// The running process still has budget.
    Continue(ProcessId),
    /// The caller should arrange a safe scheduling point.
    QuantumExpired(ProcessId),
}

/// Explicit work/idle choice. Idle is kernel work, never an allocated process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleDecision {
    /// Run the selected process; a hardware switch remains the caller's job.
    Run(ProcessId),
    /// No runnable task exists; arrange event/deadline wakeups before CPU sleep.
    Idle {
        /// Earliest sleeping task deadline, or `None` for event-only/empty idle.
        next_deadline: Option<u64>,
    },
}

/// Allocation-free cooperative scheduler. Mutate only through an exclusive owner.
pub struct Scheduler<const N: usize> {
    table: ProcessTable<N>,
    ready: RunQueue<N>,
    current: Option<ProcessId>,
    quantum: u32,
    remaining: u32,
    now: u64,
    policy: SchedulingPolicy,
    idle_ticks: u64,
}

impl<const N: usize> Scheduler<N> {
    /// Construct an empty scheduler.
    pub const fn new() -> Self {
        Self {
            table: ProcessTable::new(),
            ready: RunQueue::new(),
            current: None,
            quantum: 4,
            remaining: 0,
            now: 0,
            policy: SchedulingPolicy::RoundRobin,
            idle_ticks: 0,
        }
    }

    /// Construct a scheduler with a checked, nonzero tick budget.
    pub fn with_quantum(quantum: u32) -> Result<Self, ProcessError> {
        if quantum == 0 {
            return Err(ProcessError::InvalidQuantum);
        }
        let mut scheduler = Self::new();
        scheduler.quantum = quantum;
        Ok(scheduler)
    }

    /// Construct with an explicit monotonic epoch and nonzero slice length.
    pub fn with_clock(quantum: u32, now: u64) -> Result<Self, ProcessError> {
        let mut scheduler = Self::with_quantum(quantum)?;
        scheduler.now = now;
        Ok(scheduler)
    }

    /// Current monotonic tick. This is not wall-clock time.
    pub const fn now(&self) -> u64 {
        self.now
    }

    /// Sleep the running process for a positive, checked number of ticks.
    pub fn sleep_for(&mut self, ticks: u64) -> Result<Option<ProcessId>, ProcessError> {
        self.current.ok_or(ProcessError::NoCurrent)?;
        let deadline = self
            .now
            .checked_add(ticks)
            .ok_or(ProcessError::ClockOverflow)?;
        self.sleep_until(deadline)
    }

    /// Sleep until a strictly future deadline and dispatch another ready task.
    pub fn sleep_until(&mut self, deadline: u64) -> Result<Option<ProcessId>, ProcessError> {
        self.current.ok_or(ProcessError::NoCurrent)?;
        if deadline <= self.now {
            return Err(ProcessError::InvalidDeadline);
        }
        let id = self.stop_current(ProcessState::Sleeping)?;
        self.table.get_mut(id).unwrap().deadline = Some(deadline);
        let stats = &mut self.table.get_mut(id).unwrap().stats;
        stats.sleeps = stats.sleeps.saturating_add(1);
        Ok(self.dispatch())
    }

    fn wake_sleepers(&mut self) {
        loop {
            let next = self
                .table
                .slots
                .iter()
                .flatten()
                .filter(|p| {
                    p.state == ProcessState::Sleeping && p.deadline.map_or(false, |d| d <= self.now)
                })
                .min_by_key(|p| (p.deadline, p.id))
                .map(|p| p.id);
            let id = match next {
                Some(id) => id,
                None => break,
            };
            self.make_ready(id);
        }
    }

    /// Remaining ticks in the current process's slice, or zero while idle.
    pub const fn remaining_quantum(&self) -> u32 {
        self.remaining
    }

    /// Consume one tick without automatically changing the running process.
    pub fn tick(&mut self) -> Result<TickOutcome, ProcessError> {
        self.now = self.now.checked_add(1).ok_or(ProcessError::ClockOverflow)?;
        for process in self.table.slots.iter_mut().flatten() {
            if process.state == ProcessState::Running {
                process.stats.runtime_ticks = process.stats.runtime_ticks.saturating_add(1);
            } else if process.state == ProcessState::Ready {
                process.stats.ready_ticks = process.stats.ready_ticks.saturating_add(1);
            }
        }
        self.wake_sleepers();
        match self.current {
            None => {
                self.idle_ticks = self.idle_ticks.saturating_add(1);
                Ok(TickOutcome::Idle)
            }
            Some(id) => {
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    Ok(TickOutcome::QuantumExpired(id))
                } else {
                    Ok(TickOutcome::Continue(id))
                }
            }
        }
    }

    /// Admit one new ready process. Rejection leaves the queue unchanged.
    pub fn spawn(&mut self) -> Result<ProcessId, ProcessError> {
        let id = self.table.spawn()?;
        // Queue capacity equals table capacity and only ready tasks are queued.
        self.ready.enqueue(id).unwrap();
        Ok(id)
    }

    /// Admit a ready task with an explicit checked priority.
    pub fn spawn_with_priority(&mut self, priority: Priority) -> Result<ProcessId, ProcessError> {
        let id = self.spawn()?;
        self.table.get_mut(id).unwrap().priority = priority;
        Ok(id)
    }

    /// Admit a child of a live parent, inheriting its configured priority.
    pub fn spawn_child(&mut self, parent: ProcessId) -> Result<ProcessId, ProcessError> {
        let process = self.table.get(parent)?;
        if process.state.is_terminal() {
            return Err(ProcessError::InvalidParent);
        }
        let priority = process.priority;
        let id = self.spawn_with_priority(priority)?;
        self.table.get_mut(id).unwrap().parent = Some(parent);
        Ok(id)
    }

    /// Reap an already-exited child, otherwise atomically block its running parent.
    /// Reinvoke after the child-exit wakeup to collect the retained exit snapshot.
    pub fn wait_child(&mut self, child: ProcessId) -> Result<WaitOutcome, ProcessError> {
        let parent = self.current.ok_or(ProcessError::NoCurrent)?;
        let process = self.table.get(child)?;
        if process.parent != Some(parent) {
            return Err(ProcessError::NotChild);
        }
        if process.state.is_terminal() {
            return self.reap(child).map(WaitOutcome::Reaped);
        }
        self.stop_current(ProcessState::Blocked)?;
        let process = self.table.get_mut(parent).unwrap();
        process.waiting_child = Some(child);
        process.stats.blocks = process.stats.blocks.saturating_add(1);
        Ok(WaitOutcome::Blocked {
            next: self.dispatch(),
        })
    }

    /// Select future dispatch policy and reset accumulated aging credits.
    /// This does not interrupt the running task or reorder FIFO admission ties.
    pub fn set_policy(&mut self, policy: SchedulingPolicy) {
        self.policy = policy;
        for process in self.table.slots.iter_mut().flatten() {
            process.missed_dispatches = 0;
        }
    }

    /// Change a nonterminal task's priority without preempting the current task.
    pub fn set_priority(&mut self, id: ProcessId, priority: Priority) -> Result<(), ProcessError> {
        let process = self.table.get_mut(id)?;
        if process.state.is_terminal() {
            return Err(ProcessError::InvalidTransition);
        }
        process.priority = priority;
        Ok(())
    }

    /// Store caller-supplied context data for a live task, not CPU registers.
    pub fn set_context(
        &mut self,
        id: ProcessId,
        context: SavedContext,
    ) -> Result<(), ProcessError> {
        let process = self.table.get_mut(id)?;
        if process.state.is_terminal() {
            return Err(ProcessError::InvalidTransition);
        }
        process.context = Some(context);
        Ok(())
    }

    fn select_next(&mut self) -> Option<ProcessId> {
        let id = match self.policy {
            SchedulingPolicy::RoundRobin => self.ready.pop()?,
            SchedulingPolicy::PriorityAging => {
                let best = self.ready.iter().min_by_key(|id| {
                    let process = self.table.get(*id).unwrap();
                    process.priority.aged(process.missed_dispatches)
                })?;
                self.ready.remove(best);
                best
            }
        };
        for waiting in self.ready.iter() {
            let process = self.table.get_mut(waiting).unwrap();
            process.missed_dispatches = process.missed_dispatches.saturating_add(1);
        }
        self.table.get_mut(id).unwrap().missed_dispatches = 0;
        Some(id)
    }

    /// Read-only access to retained process metadata.
    pub const fn processes(&self) -> &ProcessTable<N> {
        &self.table
    }

    /// Identity selected as running, if any.
    pub const fn current(&self) -> Option<ProcessId> {
        self.current
    }

    /// Number of ready processes, excluding the running process.
    pub const fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// Read ready identities in FIFO admission order, not priority selection order.
    pub fn ready_ids(&self) -> impl Iterator<Item = ProcessId> + '_ {
        self.ready.iter()
    }

    /// Check identity, queue, lifecycle, family, wait, exit, and slice invariants.
    /// This allocation-free O(N²) diagnostic is intended for selftests, not IRQs.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        if self.table.ids.next == Some(0) {
            return Err(InvariantError::Identity);
        }
        let running = self
            .table
            .iter()
            .filter(|p| p.state == ProcessState::Running)
            .count();
        if running != usize::from(self.current.is_some())
            || self.current.map_or(false, |id| {
                self.table
                    .get(id)
                    .map_or(true, |p| p.state != ProcessState::Running)
            })
        {
            return Err(InvariantError::Current);
        }
        if self.quantum == 0
            || self.remaining > self.quantum
            || (self.current.is_none() && self.remaining != 0)
        {
            return Err(InvariantError::Quantum);
        }
        for id in self.ready.iter() {
            if self
                .table
                .get(id)
                .map_or(true, |p| p.state != ProcessState::Ready)
            {
                return Err(InvariantError::ReadyQueue);
            }
        }
        for process in self.table.iter() {
            if process.id.get() == 0
                || self.table.iter().filter(|p| p.id == process.id).count() != 1
                || self
                    .table
                    .ids
                    .next
                    .map_or(false, |next| next <= process.id.get())
            {
                return Err(InvariantError::Identity);
            }
            let admissions = self.ready.iter().filter(|id| *id == process.id).count();
            if admissions != usize::from(process.state == ProcessState::Ready) {
                return Err(InvariantError::ReadyQueue);
            }
            if process.exit_code.is_some() != process.state.is_terminal() {
                return Err(InvariantError::ExitMetadata);
            }
            match process.state {
                ProcessState::Blocked => {
                    if process.deadline.is_some()
                        || process.wait.is_some() == process.waiting_child.is_some()
                    {
                        return Err(InvariantError::WaitMetadata);
                    }
                }
                ProcessState::Sleeping => {
                    if process.wait.is_some()
                        || process.waiting_child.is_some()
                        || process.deadline.map_or(true, |d| d <= self.now)
                    {
                        return Err(InvariantError::WaitMetadata);
                    }
                }
                _ => {
                    if process.wait.is_some()
                        || process.waiting_child.is_some()
                        || process.deadline.is_some()
                    {
                        return Err(InvariantError::WaitMetadata);
                    }
                }
            }
            if let Some(parent) = process.parent {
                if parent >= process.id
                    || self
                        .table
                        .get(parent)
                        .map_or(true, |p| p.state.is_terminal())
                {
                    return Err(InvariantError::Parent);
                }
            }
            if let Some(child) = process.waiting_child {
                if self.table.get(child).map_or(true, |p| {
                    p.parent != Some(process.id) || p.state.is_terminal()
                }) {
                    return Err(InvariantError::Parent);
                }
            }
        }
        Ok(())
    }

    /// Number of successfully delivered ticks without a running process.
    pub const fn idle_ticks(&self) -> u64 {
        self.idle_ticks
    }

    /// Earliest pending sleep deadline, independent of whether the CPU is idle.
    pub fn next_wake_deadline(&self) -> Option<u64> {
        self.table
            .slots
            .iter()
            .flatten()
            .filter(|p| p.state == ProcessState::Sleeping)
            .filter_map(|p| p.deadline)
            .min()
    }

    /// Dispatch if possible, otherwise return an explicit idle/deadline decision.
    /// This method does not mask interrupts, program a timer, or execute WFI.
    pub fn schedule(&mut self) -> ScheduleDecision {
        match self.dispatch() {
            Some(id) => ScheduleDecision::Run(id),
            None => ScheduleDecision::Idle {
                next_deadline: self.next_wake_deadline(),
            },
        }
    }

    /// Select a ready process using policy only when no process is running.
    pub fn dispatch(&mut self) -> Option<ProcessId> {
        if self.current.is_none() {
            if let Some(id) = self.select_next() {
                self.table.transition(id, ProcessState::Running).unwrap();
                let stats = &mut self.table.get_mut(id).unwrap().stats;
                stats.dispatches = stats.dispatches.saturating_add(1);
                self.current = Some(id);
                self.remaining = self.quantum;
            }
        }
        self.current
    }

    /// Block on an event and immediately select another ready process if possible.
    /// The caller must atomically coordinate condition checks and event delivery.
    pub fn block_current(
        &mut self,
        channel: WaitChannel,
    ) -> Result<Option<ProcessId>, ProcessError> {
        let id = self.stop_current(ProcessState::Blocked)?;
        self.table.get_mut(id).unwrap().wait = Some(channel);
        let stats = &mut self.table.get_mut(id).unwrap().stats;
        stats.blocks = stats.blocks.saturating_add(1);
        Ok(self.dispatch())
    }

    /// Make one event-blocked process ready without implicitly dispatching it.
    pub fn wake(&mut self, id: ProcessId) -> Result<(), ProcessError> {
        let process = self.table.get(id)?;
        if process.state != ProcessState::Blocked || process.wait.is_none() {
            return Err(ProcessError::InvalidTransition);
        }
        self.make_ready(id);
        Ok(())
    }

    fn make_ready(&mut self, id: ProcessId) {
        self.table.transition(id, ProcessState::Ready).unwrap();
        let process = self.table.get_mut(id).unwrap();
        process.wait = None;
        process.waiting_child = None;
        process.deadline = None;
        process.stats.wakeups = process.stats.wakeups.saturating_add(1);
        self.ready.enqueue(id).unwrap();
    }

    /// Wake the lowest-identity waiter matching this channel, if any.
    pub fn wake_one(&mut self, channel: WaitChannel) -> Option<ProcessId> {
        let id = self
            .table
            .slots
            .iter()
            .flatten()
            .filter(|p| p.state == ProcessState::Blocked && p.wait == Some(channel))
            .map(|p| p.id)
            .min()?;
        self.wake(id).unwrap();
        Some(id)
    }

    /// Wake every matching waiter in ascending identity order; return the count.
    pub fn wake_all(&mut self, channel: WaitChannel) -> usize {
        let mut count = 0;
        while self.wake_one(channel).is_some() {
            count += 1;
        }
        count
    }

    /// End the current process, retaining its slot and status until explicit reap.
    pub fn exit_current(&mut self, code: i32) -> Result<Option<ProcessId>, ProcessError> {
        let id = self.stop_current(ProcessState::Zombie)?;
        let process = self.table.get_mut(id).unwrap();
        process.exit_code = Some(code);
        process.wait = None;
        process.deadline = None;
        process.waiting_child = None;
        let parent = process.parent;
        for child in self.table.slots.iter_mut().flatten() {
            if child.parent == Some(id) {
                child.parent = None;
            }
        }
        if let Some(parent) = parent {
            if self.table.get(parent).map_or(false, |p| {
                p.state == ProcessState::Blocked && p.waiting_child == Some(id)
            }) {
                self.make_ready(parent);
            }
        }
        Ok(self.dispatch())
    }

    /// Collect an exited process's complete metadata and release only its slot.
    /// Process identities remain permanently retired, preventing stale-PID aliasing.
    /// This is privileged owner cleanup; `wait_child` enforces parent ownership.
    pub fn reap(&mut self, id: ProcessId) -> Result<Process, ProcessError> {
        if self.table.get(id)?.state() != ProcessState::Zombie {
            return Err(ProcessError::NotZombie);
        }
        let slot = self
            .table
            .slots
            .iter_mut()
            .find(|slot| slot.map_or(false, |p| p.id == id))
            .unwrap();
        Ok(slot.take().unwrap())
    }

    fn stop_current(&mut self, next: ProcessState) -> Result<ProcessId, ProcessError> {
        let id = self.current.ok_or(ProcessError::NoCurrent)?;
        self.table.transition(id, next)?;
        self.current = None;
        self.remaining = 0;
        Ok(id)
    }

    /// Voluntarily donate the unused slice and rejoin at the ready queue tail.
    /// Unlike `reschedule`, yielding while idle is an error with no side effects.
    pub fn yield_current(&mut self) -> Result<Option<ProcessId>, ProcessError> {
        let id = self.stop_current(ProcessState::Ready)?;
        let stats = &mut self.table.get_mut(id).unwrap().stats;
        stats.yields = stats.yields.saturating_add(1);
        self.ready.enqueue(id).unwrap();
        Ok(self.dispatch())
    }

    /// Rotate the running task to the queue tail and dispatch the oldest ready.
    /// This changes metadata only; the caller must implement hardware switching.
    pub fn reschedule(&mut self) -> Option<ProcessId> {
        if self.current.is_some() {
            let id = self.stop_current(ProcessState::Ready).unwrap();
            self.ready.enqueue(id).unwrap();
        }
        self.dispatch()
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;
    use crate::process::TaskStats;

    #[test]
    fn diagnostics_reject_corrupted_identity_queue_current_wait_exit_and_family() {
        let make = || {
            let mut scheduler = Scheduler::<1>::new();
            let id = scheduler.spawn().unwrap();
            (scheduler, id)
        };
        let (mut scheduler, _) = make();
        scheduler.table.ids.next = Some(0);
        assert_eq!(scheduler.check_invariants(), Err(InvariantError::Identity));
        let (mut scheduler, id) = make();
        scheduler.ready.remove(id);
        assert_eq!(
            scheduler.check_invariants(),
            Err(InvariantError::ReadyQueue)
        );
        let (mut scheduler, id) = make();
        scheduler.current = Some(id);
        assert_eq!(scheduler.check_invariants(), Err(InvariantError::Current));
        let (mut scheduler, _) = make();
        scheduler.quantum = 0;
        assert_eq!(scheduler.check_invariants(), Err(InvariantError::Quantum));
        let (mut scheduler, id) = make();
        scheduler.dispatch();
        scheduler.block_current(WaitChannel::new(1)).unwrap();
        scheduler.table.get_mut(id).unwrap().wait = None;
        assert_eq!(
            scheduler.check_invariants(),
            Err(InvariantError::WaitMetadata)
        );
        let (mut scheduler, id) = make();
        scheduler.table.get_mut(id).unwrap().exit_code = Some(0);
        assert_eq!(
            scheduler.check_invariants(),
            Err(InvariantError::ExitMetadata)
        );
        let (mut scheduler, id) = make();
        scheduler.table.get_mut(id).unwrap().parent = Some(id);
        assert_eq!(scheduler.check_invariants(), Err(InvariantError::Parent));
    }

    #[test]
    fn scheduler_reports_pid_exhaustion_after_last_identity_is_reaped() {
        let mut scheduler = Scheduler::<1>::new();
        scheduler.table.ids =
            super::super::PidAllocator::starting_at(ProcessId::new(u64::MAX).unwrap());
        let last = scheduler.spawn().unwrap();
        scheduler.dispatch();
        scheduler.exit_current(0).unwrap();
        scheduler.reap(last).unwrap();
        assert_eq!(scheduler.spawn(), Err(ProcessError::IdExhausted));
        assert_eq!(scheduler.current(), None);
        assert_eq!(scheduler.ready_len(), 0);
        assert!(scheduler.processes().is_empty());
        assert_eq!(scheduler.check_invariants(), Ok(()));
    }

    #[test]
    fn task_counters_saturate_under_real_scheduler_operations() {
        let maximum = TaskStats {
            runtime_ticks: u64::MAX,
            ready_ticks: u64::MAX,
            dispatches: u64::MAX,
            yields: u64::MAX,
            blocks: u64::MAX,
            sleeps: u64::MAX,
            wakeups: u64::MAX,
        };
        let mut scheduler = Scheduler::<2>::new();
        let a = scheduler.spawn().unwrap();
        let b = scheduler.spawn().unwrap();
        scheduler.table.get_mut(a).unwrap().stats = maximum;
        scheduler.table.get_mut(b).unwrap().stats = maximum;
        scheduler.dispatch();
        scheduler.tick().unwrap();
        scheduler.yield_current().unwrap();
        scheduler.block_current(WaitChannel::new(1)).unwrap();
        scheduler.wake(b).unwrap();
        scheduler.sleep_for(1).unwrap();
        scheduler.tick().unwrap();
        assert_eq!(scheduler.table.get(a).unwrap().stats, maximum);
        assert_eq!(scheduler.table.get(b).unwrap().stats, maximum);
    }
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}
