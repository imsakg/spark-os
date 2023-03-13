//! ARM PL011 UART polling driver; board clock and pin routing remain external.

use super::mmio::RegisterIo;

const DR: usize = 0x00;
const FR: usize = 0x18;
const TXFF: u32 = 1 << 5;
const BUSY: u32 = 1 << 3;

/// Bounded polling failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UartError {
    /// The requested condition was not observed within the register-read budget.
    Timeout,
    /// UARTDR bits 11:8: overrun, break, parity, framing (high to low).
    /// The erroneous byte is discarded and UARTECR is cleared before returning.
    Receive(u8),
}

/// An exclusively owned PL011 register transport, not yet necessarily configured.
pub struct Pl011<T: RegisterIo> {
    io: T,
}

impl<T: RegisterIo> Pl011<T> {
    /// Bind a transport whose base is the UART register block.
    pub fn new(io: T) -> Self {
        Self { io }
    }

    /// Recover ownership of the transport without changing UART registers.
    pub fn release(self) -> T {
        self.io
    }

    /// Poll TXFF at most `polls` times, then write one raw byte to UARTDR.
    ///
    /// Zero budget performs no access. Timeout never writes UARTDR. The caller
    /// must configure clock, GPIO routing, and UARTEN/TXE before using output.
    pub fn write_byte(&mut self, byte: u8, polls: usize) -> Result<(), UartError> {
        for _ in 0..polls {
            if self.io.read32(FR) & TXFF == 0 {
                self.io.write32(DR, u32::from(byte));
                return Ok(());
            }
        }
        Err(UartError::Timeout)
    }

    /// Poll RXFE within `polls`, then consume one byte or report its error bits.
    /// A zero budget or empty FIFO never reads UARTDR.
    pub fn read_byte(&mut self, polls: usize) -> Result<u8, UartError> {
        for _ in 0..polls {
            if self.io.read32(FR) & (1 << 4) != 0 {
                continue;
            }
            let data = self.io.read32(DR);
            let errors = ((data >> 8) & 0xf) as u8;
            if errors != 0 {
                self.io.write32(0x04, 0xf);
                return Err(UartError::Receive(errors));
            }
            return Ok(data as u8);
        }
        Err(UartError::Timeout)
    }

    /// Wait for both the transmit FIFO and shift register to become idle.
    pub fn flush(&mut self, polls: usize) -> Result<(), UartError> {
        for _ in 0..polls {
            if self.io.read32(FR) & BUSY == 0 {
                return Ok(());
            }
        }
        Err(UartError::Timeout)
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn transmit_waits_for_fifo_room_and_writes_full_width_data_register() {
        let fake = FakeIo::new([Read(0x18, 1 << 5), Read(0x18, 0), Write(0, 0xe9)]);
        let mut uart = Pl011::new(fake);
        assert_eq!(uart.write_byte(0xe9, 2), Ok(()));
        uart.release().done();
    }
    #[test]
    fn transmit_budget_and_flush_are_bounded() {
        let fake = FakeIo::new([
            Read(0x18, 1 << 5),
            Read(0x18, 1 << 5),
            Read(0x18, 1 << 3),
            Read(0x18, 0),
        ]);
        let mut uart = Pl011::new(fake);
        assert_eq!(uart.write_byte(b'x', 0), Err(UartError::Timeout));
        assert_eq!(uart.write_byte(b'x', 2), Err(UartError::Timeout));
        assert_eq!(uart.flush(2), Ok(()));
        uart.release().done();
    }
}

#[cfg(test)]
mod receive_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn receive_waits_for_nonempty_fifo_and_returns_byte() {
        let mut uart = Pl011::new(FakeIo::new([
            Read(0x18, 1 << 4),
            Read(0x18, 0),
            Read(0, 0xa5),
        ]));
        assert_eq!(uart.read_byte(2), Ok(0xa5));
        uart.release().done();
    }
    #[test]
    fn receive_timeout_never_consumes_data() {
        let mut uart = Pl011::new(FakeIo::new([Read(0x18, 1 << 4)]));
        assert_eq!(uart.read_byte(0), Err(UartError::Timeout));
        assert_eq!(uart.read_byte(1), Err(UartError::Timeout));
        uart.release().done();
    }
    #[test]
    fn receive_reports_all_error_bits_and_clears_sticky_status() {
        for flags in 1_u8..=15 {
            let mut uart = Pl011::new(FakeIo::new([
                Read(0x18, 0),
                Read(0, (u32::from(flags) << 8) | 0x41),
                Write(4, 15),
            ]));
            assert_eq!(uart.read_byte(1), Err(UartError::Receive(flags)));
            uart.release().done();
        }
    }
}
