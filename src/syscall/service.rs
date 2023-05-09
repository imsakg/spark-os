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
        match request.number {
            63 => {
                let [fd, address, length, ..] = request.arguments;
                if fd != 0 {
                    return Err(Errno::BadFd);
                }
                let bytes = memory.writable(address, length)?;
                if bytes.is_empty() {
                    return Ok(0);
                }
                let count = self.console.read(bytes)?;
                if count > bytes.len() {
                    return Err(Errno::Io);
                }
                Ok(count as u64)
            }
            64 => {
                let [fd, address, length, ..] = request.arguments;
                if fd != 1 && fd != 2 {
                    return Err(Errno::BadFd);
                }
                let bytes = memory.read(address, length)?;
                if bytes.is_empty() {
                    return Ok(0);
                }
                let count = self.console.write(bytes)?;
                if count > bytes.len() {
                    return Err(Errno::Io);
                }
                Ok(count as u64)
            }
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
mod read_tests {
    use super::*;
    struct Input {
        calls: usize,
    }
    impl ConsoleIo for Input {
        fn write(&mut self, _: &[u8]) -> Result<usize, Errno> {
            Err(Errno::BadFd)
        }
        fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Errno> {
            self.calls += 1;
            if self.calls > 1 {
                return Err(Errno::Again);
            }
            bytes[..2].copy_from_slice(&[0xff, 0]);
            Ok(2)
        }
    }
    #[test]
    fn read_validates_full_buffer_before_consuming_input_and_distinguishes_again() {
        let mut scheduler = Scheduler::<1>::new();
        scheduler.spawn().unwrap();
        scheduler.dispatch();
        let mut console = Input { calls: 0 };
        let mut bytes = [5; 4];
        let mut memory = UserMemory::new(0x1000, &mut bytes).unwrap();
        let mut services = Services::new(&mut scheduler, &mut console);
        let request = Request {
            number: 63,
            arguments: [0, 0x1000, 4, 0, 0, 0],
        };
        assert_eq!(
            services.dispatch(
                Request {
                    arguments: [0, 0x1003, 2, 0, 0, 0],
                    ..request
                },
                &mut memory
            ),
            Err(Errno::Fault)
        );
        assert_eq!(services.dispatch(request, &mut memory), Ok(2));
        assert_eq!(memory.read(0x1000, 4), Ok(&[0xff, 0, 5, 5][..]));
        assert_eq!(services.dispatch(request, &mut memory), Err(Errno::Again));
        assert_eq!(
            services.dispatch(
                Request {
                    arguments: [1, 0x1000, 4, 0, 0, 0],
                    ..request
                },
                &mut memory
            ),
            Err(Errno::BadFd)
        );
        assert_eq!(console.calls, 2);
    }
}

#[cfg(test)]
mod write_tests {
    use super::{tests::Console, *};
    #[test]
    fn write_preserves_binary_data_and_reports_partial_consumption() {
        let mut scheduler = Scheduler::<1>::new();
        scheduler.spawn().unwrap();
        scheduler.dispatch();
        let mut console = Console {
            bytes: std::vec::Vec::new(),
            limit: 3,
        };
        let mut bytes = [0, 0xff, 0xc3, 0xa9];
        let mut memory = UserMemory::new(0x1000, &mut bytes).unwrap();
        let mut services = Services::new(&mut scheduler, &mut console);
        assert_eq!(
            services.dispatch(
                Request {
                    number: 64,
                    arguments: [1, 0x1000, 4, 0, 0, 0]
                },
                &mut memory
            ),
            Ok(3)
        );
        assert_eq!(
            services.dispatch(
                Request {
                    number: 64,
                    arguments: [2, 0x1000, 0, 0, 0, 0]
                },
                &mut memory
            ),
            Ok(0)
        );
        assert_eq!(
            services.dispatch(
                Request {
                    number: 64,
                    arguments: [0, 0x1000, 4, 0, 0, 0]
                },
                &mut memory
            ),
            Err(Errno::BadFd)
        );
        assert_eq!(
            services.dispatch(
                Request {
                    number: 64,
                    arguments: [1, 0x1003, 2, 0, 0, 0]
                },
                &mut memory
            ),
            Err(Errno::Fault)
        );
        assert_eq!(console.bytes, [0, 0xff, 0xc3]);
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
