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
    /// Clock/baud is zero or cannot be represented by PL011 divisors.
    InvalidBaud,
    /// UARTDR bits 11:8: overrun, break, parity, framing (high to low).
    /// The erroneous byte is discarded and UARTECR is cleared before returning.
    Receive(u8),
}

/// A text operation failed after an exact count of bytes had reached UARTDR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialWrite {
    /// Successfully emitted bytes, including inserted carriage returns.
    pub written: usize,
    /// The first failed byte's device error.
    pub error: UartError,
}

/// Checked integer and six-bit fractional baud divisors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BaudDivisors {
    /// UARTIBRD, in the inclusive range 1..=65535.
    pub integer: u16,
    /// UARTFBRD, in the inclusive range 0..=63.
    pub fractional: u8,
}

impl BaudDivisors {
    /// Round `clock_hz / (16 * baud)` to the nearest 1/64, carrying into IBRD.
    /// The clock must be the actual UART input clock, not the CPU/core clock.
    pub fn calculate(clock_hz: u32, baud: u32) -> Result<Self, UartError> {
        if clock_hz == 0 || baud == 0 {
            return Err(UartError::InvalidBaud);
        }
        let scaled = (u64::from(clock_hz) * 4 + u64::from(baud) / 2) / u64::from(baud);
        if scaled < 64 || scaled / 64 > u64::from(u16::MAX) {
            return Err(UartError::InvalidBaud);
        }
        Ok(Self {
            integer: (scaled / 64) as u16,
            fractional: (scaled % 64) as u8,
        })
    }
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

    /// Configure polling-only 8-N-1 with FIFOs, RX, and TX enabled.
    ///
    /// Invalid divisors cause no I/O. Initialization disables the UART before
    /// waiting for idle; timeout leaves it disabled. Firmware clock setup,
    /// pin muxing, and board-specific ordering remain caller responsibilities.
    pub fn initialize(&mut self, clock_hz: u32, baud: u32, polls: usize) -> Result<(), UartError> {
        let divisors = BaudDivisors::calculate(clock_hz, baud)?;
        self.io.write32(0x30, 0);
        self.flush(polls)?;
        self.io.write32(0x38, 0);
        self.io.write32(0x44, 0x7ff);
        self.io.write32(0x04, 0xf);
        self.io.write32(0x24, u32::from(divisors.integer));
        self.io.write32(0x28, u32::from(divisors.fractional));
        self.io.write32(0x2c, (3 << 5) | (1 << 4));
        self.io.write32(0x30, (1 << 9) | (1 << 8) | 1);
        Ok(())
    }

    /// Write UTF-8 bytes, expanding LF to CRLF, with accurate partial accounting.
    pub fn write_text(&mut self, text: &str, polls: usize) -> Result<usize, PartialWrite> {
        let mut written = 0;
        for &byte in text.as_bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r', polls)
                    .map_err(|error| PartialWrite { written, error })?;
                written += 1;
            }
            self.write_byte(byte, polls)
                .map_err(|error| PartialWrite { written, error })?;
            written += 1;
        }
        Ok(written)
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
mod text_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn text_is_utf8_with_crlf_and_timeouts_report_actual_partial_bytes() {
        let mut uart = Pl011::new(FakeIo::new([
            Read(0x18, 0),
            Write(0, 0xc3),
            Read(0x18, 0),
            Write(0, 0xa9),
            Read(0x18, 0),
            Write(0, 13),
            Read(0x18, 0),
            Write(0, 10),
        ]));
        assert_eq!(uart.write_text("é\n", 1), Ok(4));
        uart.release().done();
        let mut uart = Pl011::new(FakeIo::new([
            Read(0x18, 0),
            Write(0, 97),
            Read(0x18, 1 << 5),
        ]));
        assert_eq!(
            uart.write_text("ab", 1),
            Err(PartialWrite {
                written: 1,
                error: UartError::Timeout
            })
        );
        uart.release().done();
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

#[cfg(test)]
mod configuration_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn divisors_round_fractional_carry_and_reject_invalid_ranges() {
        assert_eq!(
            BaudDivisors::calculate(48_000_000, 115_200),
            Ok(BaudDivisors {
                integer: 26,
                fractional: 3
            })
        );
        assert_eq!(
            BaudDivisors::calculate(255, 16),
            Ok(BaudDivisors {
                integer: 1,
                fractional: 0
            })
        );
        for (clock, baud) in [(0, 1), (1, 0), (1, 115_200), (u32::MAX, 1)] {
            assert_eq!(
                BaudDivisors::calculate(clock, baud),
                Err(UartError::InvalidBaud)
            );
        }
    }
    #[test]
    fn configuration_programs_8n1_fifo_only_after_idle() {
        let mut uart = Pl011::new(FakeIo::new([
            Write(0x30, 0),
            Read(0x18, 1 << 3),
            Read(0x18, 0),
            Write(0x38, 0),
            Write(0x44, 0x7ff),
            Write(4, 15),
            Write(0x24, 26),
            Write(0x28, 3),
            Write(0x2c, 0x70),
            Write(0x30, 0x301),
        ]));
        assert_eq!(uart.initialize(48_000_000, 115_200, 2), Ok(()));
        uart.release().done();
    }
    #[test]
    fn invalid_baud_is_side_effect_free_and_busy_init_stays_disabled() {
        let mut uart = Pl011::new(FakeIo::new([Write(0x30, 0), Read(0x18, 1 << 3)]));
        assert_eq!(uart.initialize(0, 115_200, 1), Err(UartError::InvalidBaud));
        assert_eq!(
            uart.initialize(48_000_000, 115_200, 1),
            Err(UartError::Timeout)
        );
        uart.release().done();
    }
}
