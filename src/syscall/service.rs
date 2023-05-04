//! Services borrowed by one serialized kernel-call dispatch.

use super::{CallResult, Errno, Request, UserMemory};
use crate::process::Scheduler;

/// Nonblocking byte-stream boundary. Successful counts must not exceed the slice.
pub trait ConsoleIo {
    /// Write some bytes, or return an error when no byte was consumed.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Errno>;
    /// Read available bytes; zero means EOF and `Again` means not yet available.
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Errno>;
}

/// Exclusive borrowed kernel services for one serialized call boundary.
pub struct Services<'a, C: ConsoleIo, const N: usize> {
    scheduler: &'a mut Scheduler<N>,
    console: &'a mut C,
}

impl<'a, C: ConsoleIo, const N: usize> Services<'a, C, N> {
    /// Borrow the scheduler and console without global state or allocation.
    pub fn new(scheduler: &'a mut Scheduler<N>, console: &'a mut C) -> Self {
        Self { scheduler, console }
    }

    /// Decode a service number; unsupported numbers fail without side effects.
    pub fn dispatch(&mut self, request: Request, memory: &mut UserMemory<'_>) -> CallResult {
        let _ = (&mut self.console, memory);
        match request.number {
            172 => self
                .scheduler
                .current()
                .map(|id| id.get())
                .ok_or(Errno::NoCurrent),
            113 => Ok(self.scheduler.now()),
            _ => Err(Errno::NoSys),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process::Scheduler,
        syscall::{Errno, Request, UserMemory},
    };
    #[derive(Default)]
    pub(super) struct Console {
        pub bytes: std::vec::Vec<u8>,
        pub limit: usize,
    }
    impl ConsoleIo for Console {
        fn write(&mut self, bytes: &[u8]) -> Result<usize, Errno> {
            let n = bytes.len().min(self.limit);
            self.bytes.extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn read(&mut self, _bytes: &mut [u8]) -> Result<usize, Errno> {
            Err(Errno::Again)
        }
    }
    #[test]
    fn metadata_calls_need_current_identity_and_unknown_calls_fail_closed() {
        let mut scheduler = Scheduler::<2>::new();
        let mut console = Console::default();
        let mut backing = [];
        let mut memory = UserMemory::new(0x1000, &mut backing).unwrap();
        let request = Request {
            number: 172,
            arguments: [0; 6],
        };
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(request, &mut memory),
            Err(Errno::NoCurrent)
        );
        let id = scheduler.spawn().unwrap();
        scheduler.dispatch();
        scheduler.tick().unwrap();
        let mut services = Services::new(&mut scheduler, &mut console);
        assert_eq!(services.dispatch(request, &mut memory), Ok(id.get()));
        assert_eq!(
            services.dispatch(
                Request {
                    number: 113,
                    arguments: [0; 6]
                },
                &mut memory
            ),
            Ok(1)
        );
        assert_eq!(
            services.dispatch(
                Request {
                    number: u64::MAX,
                    arguments: [0; 6]
                },
                &mut memory
            ),
            Err(Errno::NoSys)
        );
    }
}
