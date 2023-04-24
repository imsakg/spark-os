//! BCM GPIO function selection and board-specific pull-control layouts.

use super::mmio::RegisterIo;

/// GPIO register/pin layout, independent of the mapped peripheral base.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    /// BCM2835/2836/2837: 54 pins and GPPUD/GPPUDCLK handshaking.
    Bcm2835,
    /// BCM2711: 58 pins and two-bit GPPUPPDN fields.
    Bcm2711,
}

/// A pin number outside the selected SoC layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioError {
    /// Pin number is not implemented by the selected controller.
    InvalidPin,
}

/// Checked SoC GPIO number (not a header connector pin number).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pin(u8);
impl Pin {
    /// Accept numbers 0..=57; the controller additionally checks its own limit.
    pub fn new(number: u8) -> Result<Self, GpioError> {
        if number >= 58 {
            Err(GpioError::InvalidPin)
        } else {
            Ok(Self(number))
        }
    }
    /// Return the SoC GPIO number.
    pub fn number(self) -> u8 {
        self.0
    }
}

/// Three-bit GPFSEL function encodings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Function {
    /// Input mode.
    Input = 0,
    /// Output mode.
    Output = 1,
    /// Alternate function zero (PL011 on GPIO14/15).
    Alt0 = 4,
    /// Alternate function one.
    Alt1 = 5,
    /// Alternate function two.
    Alt2 = 6,
    /// Alternate function three.
    Alt3 = 7,
    /// Alternate function four.
    Alt4 = 3,
    /// Alternate function five.
    Alt5 = 2,
}

/// Pin bias selection; hardware encodings depend on `Layout`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pull {
    /// Disable bias resistors.
    Off,
    /// Weak pull-up.
    Up,
    /// Weak pull-down.
    Down,
}

/// Serialized GPIO controller register transport.
pub struct Gpio<T: RegisterIo> {
    io: T,
    layout: Layout,
}
impl<T: RegisterIo> Gpio<T> {
    /// Bind a transport based at GPFSEL0 with an explicit SoC layout.
    pub fn new(io: T, layout: Layout) -> Self {
        Self { io, layout }
    }
    /// Recover transport ownership without changing pin configuration.
    pub fn release(self) -> T {
        self.io
    }
    fn validate(&self, pin: Pin) -> Result<usize, GpioError> {
        let limit = match self.layout {
            Layout::Bcm2835 => 54,
            Layout::Bcm2711 => 58,
        };
        if pin.0 >= limit {
            Err(GpioError::InvalidPin)
        } else {
            Ok(usize::from(pin.0))
        }
    }
    /// Update one GPFSEL field, preserving all neighboring pins.
    /// The caller must serialize configuration against other cores/interrupts.
    pub fn set_function(&mut self, pin: Pin, function: Function) -> Result<(), GpioError> {
        let pin = self.validate(pin)?;
        let register = (pin / 10) * 4;
        let shift = (pin % 10) * 3;
        let old = self.io.read32(register);
        self.io.write32(
            register,
            (old & !(7 << shift)) | ((function as u32) << shift),
        );
        Ok(())
    }
    /// Drive one output latch using GPSET/GPCLR, without reading either register.
    /// Pin direction must already have been configured by the caller.
    pub fn write_pin(&mut self, pin: Pin, high: bool) -> Result<(), GpioError> {
        let pin = self.validate(pin)?;
        let base = if high { 0x1c } else { 0x28 };
        self.io.write32(base + (pin / 32) * 4, 1 << (pin % 32));
        Ok(())
    }

    /// Sample the physical pin level from GPLEV, independently of its function.
    pub fn is_high(&mut self, pin: Pin) -> Result<bool, GpioError> {
        let pin = self.validate(pin)?;
        Ok(self.io.read32(0x34 + (pin / 32) * 4) & (1 << (pin % 32)) != 0)
    }

    /// Configure bias using the selected layout.
    ///
    /// Legacy hardware invokes `delay` twice; each call MUST conservatively
    /// satisfy the SoC's 150-cycle GPPUD delay (the datasheet does not identify
    /// those as CPU cycles). Supply a board-qualified delay, not an empty loop.
    /// BCM2711 instead uses GPPUPPDN and never calls `delay`.
    pub fn set_pull(
        &mut self,
        pin: Pin,
        pull: Pull,
        mut delay: impl FnMut(),
    ) -> Result<(), GpioError> {
        let pin = self.validate(pin)?;
        match self.layout {
            Layout::Bcm2835 => {
                let bits = match pull {
                    Pull::Off => 0,
                    Pull::Down => 1,
                    Pull::Up => 2,
                };
                let clock = 0x98 + (pin / 32) * 4;
                self.io.write32(0x94, bits);
                delay();
                self.io.write32(clock, 1 << (pin % 32));
                delay();
                self.io.write32(0x94, 0);
                self.io.write32(clock, 0);
            }
            Layout::Bcm2711 => {
                let bits = match pull {
                    Pull::Off => 0,
                    Pull::Up => 1,
                    Pull::Down => 2,
                };
                let register = 0xe4 + (pin / 16) * 4;
                let shift = (pin % 16) * 2;
                let old = self.io.read32(register);
                self.io
                    .write32(register, (old & !(3 << shift)) | (bits << shift));
            }
        }
        Ok(())
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
    fn function_selection_preserves_neighbor_pins() {
        let mut gpio = Gpio::new(
            FakeIo::new([
                Read(4, u32::MAX),
                Write(4, (u32::MAX & !(7 << 12)) | (4 << 12)),
            ]),
            Layout::Bcm2835,
        );
        gpio.set_function(Pin::new(14).unwrap(), Function::Alt0)
            .unwrap();
        gpio.release().done();
        assert_eq!(Pin::new(58), Err(GpioError::InvalidPin));
    }
    #[test]
    fn legacy_pull_requires_two_explicit_delay_windows_and_clears_clocks() {
        let mut gpio = Gpio::new(
            FakeIo::new([
                Write(0x94, 2),
                Write(0x9c, 1 << 21),
                Write(0x94, 0),
                Write(0x9c, 0),
            ]),
            Layout::Bcm2835,
        );
        let mut delays = 0;
        gpio.set_pull(Pin::new(53).unwrap(), Pull::Up, || delays += 1)
            .unwrap();
        assert_eq!(delays, 2);
        gpio.release().done();
    }
    #[test]
    fn bcm2711_pull_encoding_and_upper_pins_differ_from_legacy() {
        let mut gpio = Gpio::new(
            FakeIo::new([
                Read(0xf0, u32::MAX),
                Write(0xf0, (u32::MAX & !(3 << 18)) | (2 << 18)),
            ]),
            Layout::Bcm2711,
        );
        gpio.set_pull(Pin::new(57).unwrap(), Pull::Down, || {
            panic!("modern layout has no legacy delay")
        })
        .unwrap();
        gpio.release().done();
        let mut legacy = Gpio::new(FakeIo::new([]), Layout::Bcm2835);
        assert_eq!(
            legacy.set_function(Pin::new(54).unwrap(), Function::Input),
            Err(GpioError::InvalidPin)
        );
        legacy.release().done();
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn bank_boundaries_use_atomic_set_and_clear_without_register_reads() {
        let mut gpio = Gpio::new(
            FakeIo::new([
                Write(0x1c, 1 << 31),
                Write(0x20, 1),
                Write(0x2c, 1 << 21),
                Read(0x34, 1 << 31),
                Read(0x38, 0),
            ]),
            Layout::Bcm2835,
        );
        gpio.write_pin(Pin::new(31).unwrap(), true).unwrap();
        gpio.write_pin(Pin::new(32).unwrap(), true).unwrap();
        gpio.write_pin(Pin::new(53).unwrap(), false).unwrap();
        assert_eq!(gpio.is_high(Pin::new(31).unwrap()), Ok(true));
        assert_eq!(gpio.is_high(Pin::new(32).unwrap()), Ok(false));
        assert_eq!(
            gpio.write_pin(Pin::new(54).unwrap(), true),
            Err(GpioError::InvalidPin)
        );
        gpio.release().done();
    }
    #[test]
    fn bcm2711_upper_pin_uses_bank_one() {
        let mut gpio = Gpio::new(
            FakeIo::new([Write(0x2c, 1 << 25), Read(0x38, 1 << 25)]),
            Layout::Bcm2711,
        );
        gpio.write_pin(Pin::new(57).unwrap(), false).unwrap();
        assert_eq!(gpio.is_high(Pin::new(57).unwrap()), Ok(true));
        gpio.release().done();
    }
}
