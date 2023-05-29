//! End-to-end service checks over real policy objects and in-memory pipe storage.

use super::{Errno, Request, Services, UserMemory};
use crate::{ipc::Pipe, process::Scheduler};

/// Exercise syscall dispatch, user copies, pipe I/O, scheduling and child status.
/// All state is local. This invokes no SVC instruction and installs no EL0 code.
pub fn self_test() -> Result<(), &'static str> {
    let mut scheduler = Scheduler::<2>::new();
    let parent = scheduler.spawn().map_err(|_| "parent spawn")?;
    let child = scheduler.spawn_child(parent).map_err(|_| "child spawn")?;
    scheduler.dispatch();
    let mut pipe = Pipe::<8>::new();
    let mut backing = [0u8; 12];
    backing[..4].copy_from_slice(&[0, 0xff, 1, 2]);
    let mut memory = UserMemory::new(0x1000, &mut backing).map_err(|_| "admit buffer")?;
    let mut services = Services::new(&mut scheduler, &mut pipe);
    let calls = [
        (64, [1, 0x1000, 4, 0, 0, 0], Ok(4)),
        (63, [0, 0x1004, 4, 0, 0, 0], Ok(4)),
        (172, [0; 6], Ok(parent.get())),
        (124, [0; 6], Ok(0)),
        (172, [0; 6], Ok(child.get())),
        (101, [1, 0, 0, 0, 0, 0], Ok(0)),
        (u64::MAX, [0; 6], Err(Errno::NoSys)),
    ];
    for (number, arguments, expected) in calls {
        if services.dispatch(Request { number, arguments }, &mut memory) != expected {
            return Err("service dispatch");
        }
    }
    if memory.read(0x1004, 4).map_err(|_| "copy result")? != [0, 0xff, 1, 2] {
        return Err("binary copy");
    }
    scheduler.tick().map_err(|_| "clock tick")?;
    scheduler.yield_current().map_err(|_| "resume child")?;
    let mut services = Services::new(&mut scheduler, &mut pipe);
    if services.dispatch(
        Request {
            number: 93,
            arguments: [7, 0, 0, 0, 0, 0],
        },
        &mut memory,
    ) != Ok(0)
        || services.dispatch(
            Request {
                number: 260,
                arguments: [child.get(), 0x1008, 0, 0, 0, 0],
            },
            &mut memory,
        ) != Ok(child.get())
    {
        return Err("exit wait");
    }
    if memory.read(0x1008, 4).map_err(|_| "exit copy")? != 7_i32.to_le_bytes() {
        return Err("exit status");
    }
    scheduler
        .check_invariants()
        .map_err(|_| "scheduler invariants")?;
    if pipe.queued_bytes() != 0 {
        return Err("pipe drain");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_selftest_is_repeatable_without_hardware_or_global_state() {
        assert_eq!(super::self_test(), Ok(()));
        assert_eq!(super::self_test(), Ok(()));
    }
}
