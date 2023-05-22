//! Services borrowed by one serialized kernel-call dispatch.

use super::{CallResult, Errno, Request, UserMemory};
use crate::process::{ProcessError, Scheduler};

impl From<ProcessError> for Errno {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::NoCurrent | ProcessError::NotFound => Self::NoCurrent,
            ProcessError::ClockOverflow | ProcessError::IdExhausted => Self::Overflow,
            ProcessError::TableFull => Self::Again,
            ProcessError::NotChild => Self::NoChild,
            _ => Self::Invalid,
        }
    }
}

/// Maximum bytes processed by one console call to bound synchronous kernel work.
pub const MAX_IO_BYTES: u64 = 4096;

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
        if matches!(request.number, 63 | 64 | 93 | 101 | 124 | 172 | 260)
            && self.scheduler.current().is_none()
        {
            return Err(Errno::NoCurrent);
        }
        if matches!(request.number, 63 | 64) && request.arguments[2] > MAX_IO_BYTES {
            return Err(Errno::Invalid);
        }
        match request.number {
            93 => {
                let code =
                    i32::try_from(request.arguments[0] as i64).map_err(|_| Errno::Invalid)?;
                self.scheduler.exit_current(code).map_err(Errno::from)?;
                Ok(0)
            }
            260 => {
                let parent = self.scheduler.current().ok_or(Errno::NoCurrent)?;
                let child =
                    crate::process::ProcessId::new(request.arguments[0]).ok_or(Errno::NoChild)?;
                let process = self
                    .scheduler
                    .processes()
                    .get(child)
                    .map_err(|_| Errno::NoChild)?;
                if process.parent() != Some(parent) {
                    return Err(Errno::NoChild);
                }
                // Validate output before collecting the only retained status snapshot.
                let output = memory.writable(request.arguments[1], 4)?;
                let code = process.exit_code().ok_or(Errno::Again)?;
                self.scheduler.reap(child).map_err(Errno::from)?;
                output.copy_from_slice(&code.to_le_bytes());
                Ok(child.get())
            }
            124 => {
                self.scheduler.yield_current().map_err(Errno::from)?;
                Ok(0)
            }
            101 => {
                self.scheduler
                    .sleep_for(request.arguments[0])
                    .map_err(Errno::from)?;
                Ok(0)
            }
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
mod boundary_tests {
    use super::{tests::Console, *};
    #[test]
    fn effectful_calls_need_a_current_process_and_io_has_a_fixed_work_budget() {
        let mut scheduler = Scheduler::<1>::new();
        let mut console = Console {
            bytes: std::vec::Vec::new(),
            limit: usize::MAX,
        };
        let mut backing = [0u8; 8];
        let mut memory = UserMemory::new(0x1000, &mut backing).unwrap();
        for number in [63, 64, 93, 101, 124, 172, 260] {
            assert_eq!(
                Services::new(&mut scheduler, &mut console).dispatch(
                    Request {
                        number,
                        arguments: [1, 0x1000, 1, 0, 0, 0]
                    },
                    &mut memory
                ),
                Err(Errno::NoCurrent)
            );
        }
        scheduler.spawn().unwrap();
        scheduler.dispatch();
        for number in [63, 64] {
            assert_eq!(
                Services::new(&mut scheduler, &mut console).dispatch(
                    Request {
                        number,
                        arguments: [u64::from(number == 64), 0x1000, 4097, 0, 0, 0]
                    },
                    &mut memory
                ),
                Err(Errno::Invalid)
            );
        }
        for number in [0, 1, 62, 65, 112, 125, u64::MAX] {
            assert_eq!(
                Services::new(&mut scheduler, &mut console).dispatch(
                    Request {
                        number,
                        arguments: [u64::MAX; 6]
                    },
                    &mut memory
                ),
                Err(Errno::NoSys)
            );
        }
        assert!(console.bytes.is_empty());
        assert_eq!(scheduler.check_invariants(), Ok(()));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{tests::Console, *};
    #[test]
    fn wait_checks_parent_and_status_buffer_before_collecting_child() {
        let mut scheduler = Scheduler::<2>::new();
        let parent = scheduler.spawn().unwrap();
        let child = scheduler.spawn_child(parent).unwrap();
        scheduler.dispatch();
        let mut console = Console::default();
        let mut backing = [0xaa; 4];
        let mut memory = UserMemory::new(0x1000, &mut backing).unwrap();
        let wait = Request {
            number: 260,
            arguments: [child.get(), 0x1000, 0, 0, 0, 0],
        };
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(wait, &mut memory),
            Err(Errno::Again)
        );
        assert_eq!(scheduler.current(), Some(parent));
        scheduler.yield_current().unwrap();
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(
                Request {
                    number: 93,
                    arguments: [(-7_i64) as u64, 0, 0, 0, 0, 0]
                },
                &mut memory
            ),
            Ok(0)
        );
        assert_eq!(scheduler.current(), Some(parent));
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(
                Request {
                    arguments: [child.get(), 0x1001, 0, 0, 0, 0],
                    ..wait
                },
                &mut memory
            ),
            Err(Errno::Fault)
        );
        assert!(scheduler.processes().get(child).is_ok());
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(wait, &mut memory),
            Ok(child.get())
        );
        assert_eq!(memory.read(0x1000, 4), Ok(&(-7_i32).to_le_bytes()[..]));
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(wait, &mut memory),
            Err(Errno::NoChild)
        );
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::{tests::Console, *};
    #[test]
    fn yield_and_sleep_change_policy_without_claiming_cpu_context_switches() {
        let mut scheduler = Scheduler::<2>::with_quantum(1).unwrap();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();
        scheduler.dispatch();
        let mut console = Console::default();
        let mut backing = [];
        let mut memory = UserMemory::new(0, &mut backing).unwrap();
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(
                Request {
                    number: 124,
                    arguments: [0; 6]
                },
                &mut memory
            ),
            Ok(0)
        );
        assert_eq!(scheduler.current(), Some(second));
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(
                Request {
                    number: 101,
                    arguments: [2, 0, 0, 0, 0, 0]
                },
                &mut memory
            ),
            Ok(0)
        );
        assert_eq!(scheduler.current(), Some(first));
        assert_eq!(scheduler.next_wake_deadline(), Some(2));
        assert_eq!(
            Services::new(&mut scheduler, &mut console).dispatch(
                Request {
                    number: 101,
                    arguments: [0; 6]
                },
                &mut memory
            ),
            Err(Errno::Invalid)
        );
        scheduler.tick().unwrap();
        scheduler.tick().unwrap();
        scheduler.yield_current().unwrap();
        assert_eq!(scheduler.current(), Some(second));
        assert_eq!(scheduler.check_invariants(), Ok(()));
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
