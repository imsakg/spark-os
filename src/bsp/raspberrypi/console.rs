//! BSP console facilities.

use crate::{console, synchronization, synchronization::NullLock};
use core::fmt;
use spark_os::{
    drivers::{
        mmio::VolatileMmio,
        pl011::{Pl011, UartError},
    },
    syscall::{ConsoleIo, Errno},
};

#[cfg(feature = "bsp_rpi3")]
const UART_BASE: usize = 0x3f20_1000;
#[cfg(feature = "bsp_rpi4")]
const UART_BASE: usize = 0xfe20_1000;
const POLL_BUDGET: usize = 1_000_000;

//--------------------------------------------------------------------------------------------------
// Private Definitions
//--------------------------------------------------------------------------------------------------

/// Single-boot-core polling console; interrupts must remain disabled.
///
/// The mutex protected part.
struct QEMUOutputInner {
    chars_written: usize,
    initialized: bool,
}

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// The main struct.
pub struct QEMUOutput {
    inner: NullLock<QEMUOutputInner>,
}

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

static QEMU_OUTPUT: QEMUOutput = QEMUOutput::new();

//--------------------------------------------------------------------------------------------------
// Private Code
//--------------------------------------------------------------------------------------------------

impl QEMUOutputInner {
    const fn new() -> QEMUOutputInner {
        QEMUOutputInner {
            chars_written: 0,
            initialized: false,
        }
    }

    fn with_uart<R>(&mut self, f: impl FnOnce(&mut Pl011<VolatileMmio>) -> R) -> R {
        // Safety: board boot keeps one core active, interrupts and MMU disabled.
        // Every UART access is inside this closure under the console owner lock;
        // no transport escapes or overlaps another access to this register span.
        let io = unsafe { VolatileMmio::from_raw(UART_BASE as *mut u8, 0x48) }
            .expect("board UART mapping metadata");
        f(&mut Pl011::new(io))
    }
}

/// Implementing `core::fmt::Write` enables usage of the `format_args!` macros, which in turn are
/// used to implement the `kernel`'s `print!` and `println!` macros. By implementing `write_str()`,
/// we get `write_fmt()` automatically.
///
/// The function takes an `&mut self`, so it must be implemented for the inner struct.
///
/// See [`src/print.rs`].
///
/// [`src/print.rs`]: ../../print/index.html
impl fmt::Write for QEMUOutputInner {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if !self.initialized {
            return Err(fmt::Error);
        }
        match self.with_uart(|uart| uart.write_text(s, POLL_BUDGET)) {
            Ok(count) => {
                self.chars_written = self.chars_written.saturating_add(count);
                Ok(())
            }
            Err(failure) => {
                self.chars_written = self.chars_written.saturating_add(failure.written);
                Err(fmt::Error)
            }
        }
    }
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

impl QEMUOutput {
    /// Create a new instance.
    pub const fn new() -> QEMUOutput {
        QEMUOutput {
            inner: NullLock::new(QEMUOutputInner::new()),
        }
    }
}

/// Initialize polling 8-N-1 UART using the supplied actual peripheral clock.
///
/// # Safety
/// Call only on the boot core with other cores parked, MMU and interrupts off.
/// Firmware clock and pin mux configuration must already match this board.
pub unsafe fn initialize(clock_hz: u32) -> Result<(), UartError> {
    QEMU_OUTPUT.inner.lock(|inner| {
        inner.with_uart(|uart| uart.initialize(clock_hz, 115_200, POLL_BUDGET))?;
        inner.initialized = true;
        Ok(())
    })
}

/// Raw byte-stream adapter used by kernel calls; text conversion is not applied.
pub struct SyscallConsole;
impl ConsoleIo for SyscallConsole {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Errno> {
        QEMU_OUTPUT.inner.lock(|inner| {
            if !inner.initialized {
                return Err(Errno::Io);
            }
            let count = inner.with_uart(|uart| {
                let mut count = 0;
                for &byte in bytes {
                    if uart.write_byte(byte, POLL_BUDGET).is_err() {
                        break;
                    }
                    count += 1;
                }
                count
            });
            inner.chars_written = inner.chars_written.saturating_add(count);
            if count == 0 && !bytes.is_empty() {
                Err(Errno::Again)
            } else {
                Ok(count)
            }
        })
    }
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Errno> {
        QEMU_OUTPUT.inner.lock(|inner| {
            if !inner.initialized {
                return Err(Errno::Io);
            }
            inner.with_uart(|uart| {
                let mut count = 0;
                for byte in bytes {
                    match uart.read_byte(1) {
                        Ok(value) => {
                            *byte = value;
                            count += 1;
                        }
                        Err(UartError::Timeout) if count == 0 => return Err(Errno::Again),
                        Err(_) if count == 0 => return Err(Errno::Io),
                        Err(_) => break,
                    }
                }
                Ok(count)
            })
        })
    }
}

/// Return a reference to the console.
pub fn console() -> &'static dyn console::interface::All {
    &QEMU_OUTPUT
}

//------------------------------------------------------------------------------
// OS Interface Code
//------------------------------------------------------------------------------
use synchronization::interface::Mutex;

/// Passthrough of `args` to the `core::fmt::Write` implementation, but guarded by a Mutex to
/// serialize access.
impl console::interface::Write for QEMUOutput {
    fn write_fmt(&self, args: core::fmt::Arguments) -> fmt::Result {
        // Fully qualified syntax for the call to `core::fmt::Write::write_fmt()` to increase
        // readability.
        self.inner.lock(|inner| fmt::Write::write_fmt(inner, args))
    }
}

impl console::interface::Statistics for QEMUOutput {
    fn chars_written(&self) -> usize {
        self.inner.lock(|inner| inner.chars_written)
    }
}

impl console::interface::All for QEMUOutput {}
