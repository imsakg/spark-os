//! Allocation-free peripheral drivers separated from board-specific MMIO mapping.

pub mod gpio;
pub mod mmio;
pub mod pl011;
pub mod ring;

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
