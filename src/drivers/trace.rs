//! Bounded diagnostic traces that forward real register accesses unchanged.

use super::mmio::RegisterIo;

/// A single completed register transaction at the transport boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterAccess {
    /// A register read and the value returned by the wrapped transport.
    Read {
        /// Byte offset from the mapped register block.
        offset: usize,
        /// Observed 32-bit value.
        value: u32,
    },
    /// A register write forwarded to the wrapped transport.
    Write {
        /// Byte offset from the mapped register block.
        offset: usize,
        /// Forwarded 32-bit value.
        value: u32,
    },
}

/// Bounded diagnostic wrapper, preserving all underlying I/O even when full.
///
/// The trace is not a simulated register bank. With a real transport it records
/// real accesses; host tests explicitly wrap a scripted fake. Recording adds
/// timing overhead and does not supply memory barriers or hardware ordering.
pub struct Trace<T: RegisterIo, const N: usize> {
    io: T,
    accesses: [RegisterAccess; N],
    len: usize,
    dropped: usize,
}
impl<T: RegisterIo, const N: usize> Trace<T, N> {
    /// Wrap a transport with capacity for the first `N` completed accesses.
    pub fn new(io: T) -> Self {
        Self {
            io,
            accesses: [RegisterAccess::Read {
                offset: 0,
                value: 0,
            }; N],
            len: 0,
            dropped: 0,
        }
    }
    /// Recorded prefix, in observed transport order.
    pub fn accesses(&self) -> &[RegisterAccess] {
        &self.accesses[..self.len]
    }
    /// Saturating count of transactions not recorded after capacity was reached.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
    /// Clear counters without changing the transport or erasing old storage.
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }
    /// Recover the underlying transport; no pending hardware action is cancelled.
    pub fn release(self) -> T {
        self.io
    }
    fn record(&mut self, access: RegisterAccess) {
        if self.len < N {
            self.accesses[self.len] = access;
            self.len += 1;
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}
impl<T: RegisterIo, const N: usize> RegisterIo for Trace<T, N> {
    fn read32(&mut self, offset: usize) -> u32 {
        let value = self.io.read32(offset);
        self.record(RegisterAccess::Read { offset, value });
        value
    }
    fn write32(&mut self, offset: usize, value: u32) {
        self.io.write32(offset, value);
        self.record(RegisterAccess::Write { offset, value });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::{
        mmio::RegisterIo,
        pl011::{Pl011, UartError},
        test_support::{
            Access::{Read, Write},
            FakeIo,
        },
    };
    #[test]
    fn wrapper_records_order_and_values_without_changing_transmitted_data() {
        let io = FakeIo::new([Read(0x18, 1 << 5), Read(0x18, 0), Write(0, 0xaa)]);
        let mut uart = Pl011::new(Trace::<_, 3>::new(io));
        uart.write_byte(0xaa, 2).unwrap();
        let trace = uart.release();
        assert_eq!(
            trace.accesses(),
            &[
                RegisterAccess::Read {
                    offset: 0x18,
                    value: 1 << 5
                },
                RegisterAccess::Read {
                    offset: 0x18,
                    value: 0
                },
                RegisterAccess::Write {
                    offset: 0,
                    value: 0xaa
                },
            ]
        );
        assert_eq!(trace.dropped(), 0);
        trace.release().done();
    }
    #[test]
    fn full_or_zero_trace_drops_records_not_underlying_io_and_timeout_never_writes() {
        let mut uart = Pl011::new(Trace::<_, 1>::new(FakeIo::new([
            Read(0x18, 1 << 5),
            Read(0x18, 1 << 5),
        ])));
        assert_eq!(uart.write_byte(1, 2), Err(UartError::Timeout));
        let trace = uart.release();
        assert_eq!(trace.accesses().len(), 1);
        assert_eq!(trace.dropped(), 1);
        trace.release().done();
        let mut zero = Trace::<_, 0>::new(FakeIo::new([Write(0, 7)]));
        zero.write32(0, 7);
        assert!(zero.accesses().is_empty());
        assert_eq!(zero.dropped(), 1);
        zero.clear();
        assert_eq!(zero.dropped(), 0);
        zero.release().done();
    }
}
