//! Allocation-free peripheral drivers separated from board-specific MMIO mapping.

pub mod gpio;
pub mod interrupts;
pub mod mailbox;
pub mod manager;
pub mod mmio;
pub mod pl011;
pub mod ring;
pub mod timer;
pub mod trace;

#[cfg(test)]
pub(crate) mod test_support {
    use super::mmio::RegisterIo;
    use std::collections::VecDeque;
    #[derive(Debug, PartialEq)]
    pub enum Access {
        Read(usize, u32),
        Write(usize, u32),
    }
    pub struct FakeIo {
        expected: VecDeque<Access>,
    }
    impl FakeIo {
        pub fn new(expected: impl IntoIterator<Item = Access>) -> Self {
            Self {
                expected: expected.into_iter().collect(),
            }
        }
        pub fn done(self) {
            assert!(
                self.expected.is_empty(),
                "unconsumed fake transactions: {:?}",
                self.expected
            );
        }
    }
    impl RegisterIo for FakeIo {
        fn read32(&mut self, offset: usize) -> u32 {
            match self.expected.pop_front() {
                Some(Access::Read(expected, value)) if offset == expected => value,
                other => panic!("unexpected fake read at {offset:#x}: {other:?}"),
            }
        }
        fn write32(&mut self, offset: usize, value: u32) {
            assert_eq!(
                self.expected.pop_front(),
                Some(Access::Write(offset, value)),
                "unexpected fake write"
            );
        }
    }
}

#[cfg(test)]
mod boot_self_test_contract {
    #[test]
    fn public_self_tests_are_repeatable_and_need_no_mmio_mapping() {
        assert_eq!(super::self_test(), Ok(()));
        assert_eq!(crate::modules::self_test(), Ok(()));
        assert_eq!(super::self_test(), Ok(()));
        assert_eq!(crate::modules::self_test(), Ok(()));
    }
}

/// Run allocation-free driver-logic checks without accessing any peripheral.
///
/// Exercises bounded queues, baud arithmetic, wrapping deadlines, and property
/// message construction. This does not claim that UART, GPIO, timer, mailbox,
/// or interrupt hardware is installed or working. It is repeatable at boot.
pub fn self_test() -> Result<(), &'static str> {
    let baud =
        pl011::BaudDivisors::calculate(48_000_000, 115_200).map_err(|_| "baud calculation")?;
    if baud.integer != 26 || baud.fractional != 3 {
        return Err("baud rounding");
    }
    let mut bytes = ring::ByteRing::<3>::new();
    if bytes.write(&[1, 2, 3, 4]) != 3 || bytes.push(4) != Err(ring::RingError::Full) {
        return Err("byte ring capacity");
    }
    if bytes.pop() != Some(1) {
        return Err("byte ring order");
    }
    bytes.push(4).map_err(|_| "byte ring wrap")?;
    let mut output = [0; 3];
    if bytes.read(&mut output) != 3 || output != [2, 3, 4] || !bytes.is_empty() {
        return Err("byte ring wrap order");
    }
    let deadline = timer::Deadline::after(u32::MAX - 2, 5).map_err(|_| "timer deadline")?;
    if deadline.target() != 2 || deadline.reached(u32::MAX) || !deadline.reached(2) {
        return Err("timer wrap comparison");
    }
    let mut message = mailbox::PropertyMessage::<12>::new().map_err(|_| "property capacity")?;
    message
        .add_tag(0x10001, 1, &[])
        .map_err(|_| "property tag")?;
    if message.words() != [28, 0, 0x10001, 4, 0, 0, 0] {
        return Err("property wire format");
    }
    if gpio::Pin::new(58).is_ok() || interrupts::Irq::new(72).is_ok() {
        return Err("device index bounds");
    }
    Ok(())
}
