//! BCM2836/2837 legacy peripheral IRQ controller, not local IRQs or GIC.

use super::mmio::RegisterIo;

/// Interrupt number validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptError {
    /// Only GPU IRQs 0..63 and basic IRQs 64..71 are represented.
    InvalidIrq,
}

/// A legacy peripheral IRQ number, excluding summary/shortcut bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Irq(u8);
impl Irq {
    /// Validate the driver's flattened 0..=71 numbering.
    pub fn new(number: u8) -> Result<Self, InterruptError> {
        if number >= 72 {
            Err(InterruptError::InvalidIrq)
        } else {
            Ok(Self(number))
        }
    }
    /// Return 0..63 for GPU banks, 64..71 for basic interrupts.
    pub fn number(self) -> u8 {
        self.0
    }
}

/// The legacy ARMCTRL peripheral controller used on BCM2836/2837.
///
/// This does not configure per-core local routing, exception vectors, DAIF, or
/// the BCM2711 GIC. Enabled masks track only IRQs enabled through this instance.
pub struct Bcm2836Interrupts<T: RegisterIo> {
    io: T,
    enabled: [u32; 3],
}
impl<T: RegisterIo> Bcm2836Interrupts<T> {
    /// Bind a transport based at IRQ_BASIC_PENDING without touching hardware.
    /// Call `disable_all` first if firmware's mask state is not known.
    pub fn new(io: T) -> Self {
        Self {
            io,
            enabled: [0; 3],
        }
    }
    /// Recover ownership, leaving hardware masks unchanged.
    pub fn release(self) -> T {
        self.io
    }
    /// Enable one IRQ with a write-one-to-set access, never a register RMW.
    pub fn enable(&mut self, irq: Irq) {
        let bank = usize::from(irq.0 / 32);
        let mask = 1 << (irq.0 % 32);
        self.io.write32(0x10 + bank * 4, mask);
        self.enabled[bank] |= mask;
    }
    /// Disable one IRQ with a write-one-to-clear mask.
    pub fn disable(&mut self, irq: Irq) {
        let bank = usize::from(irq.0 / 32);
        let mask = 1 << (irq.0 % 32);
        self.io.write32(0x1c + bank * 4, mask);
        self.enabled[bank] &= !mask;
    }
    /// Disable all implemented peripheral IRQs and clear software ownership.
    pub fn disable_all(&mut self) {
        self.io.write32(0x1c, u32::MAX);
        self.io.write32(0x20, u32::MAX);
        self.io.write32(0x24, 0xff);
        self.enabled = [0; 3];
    }
    /// Invoke at most `budget` callbacks from one owned-pending snapshot.
    ///
    /// Each IRQ appears once, in ascending number order. Basic summary/shortcut
    /// bits are ignored. Handlers must acknowledge the originating device:
    /// these pending registers are not acknowledgement registers. This method
    /// alone does not enable CPU interrupts or install an interrupt handler.
    pub fn dispatch(&mut self, budget: usize, mut handler: impl FnMut(Irq)) -> usize {
        if budget == 0 {
            return 0;
        }
        let pending = [
            self.io.read32(4),
            self.io.read32(8),
            self.io.read32(0) & 0xff,
        ];
        let mut handled = 0;
        for (bank, bits) in pending.iter().enumerate() {
            let mut bits = bits & self.enabled[bank];
            while bits != 0 && handled < budget {
                let bit = bits.trailing_zeros();
                bits &= !(1 << bit);
                handler(Irq((bank * 32 + bit as usize) as u8));
                handled += 1;
            }
        }
        handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn enable_disable_are_write_one_masks_and_dispatch_filters_ownership() {
        let fake = FakeIo::new([
            Write(0x10, 2),
            Write(0x14, 2),
            Write(0x18, 2),
            Write(0x20, 2),
            Read(4, 6),
            Read(8, 2),
            Read(0, 0x103),
        ]);
        let mut irq = Bcm2836Interrupts::new(fake);
        irq.enable(Irq::new(1).unwrap());
        irq.enable(Irq::new(33).unwrap());
        irq.enable(Irq::new(65).unwrap());
        irq.disable(Irq::new(33).unwrap());
        let mut handled = std::vec::Vec::new();
        assert_eq!(irq.dispatch(8, |irq| handled.push(irq.number())), 2);
        assert_eq!(handled, [1, 65]);
        irq.release().done();
    }
    #[test]
    fn dispatch_is_snapshot_bounded_and_reset_disables_all_banks() {
        let mut irq = Bcm2836Interrupts::new(FakeIo::new([
            Write(0x10, 1),
            Write(0x10, 2),
            Read(4, 3),
            Read(8, 0),
            Read(0, 0),
            Write(0x1c, u32::MAX),
            Write(0x20, u32::MAX),
            Write(0x24, 0xff),
        ]));
        assert_eq!(irq.dispatch(0, |_| panic!("zero budget")), 0);
        irq.enable(Irq::new(0).unwrap());
        irq.enable(Irq::new(1).unwrap());
        assert_eq!(irq.dispatch(1, |irq| assert_eq!(irq.number(), 0)), 1);
        irq.disable_all();
        irq.release().done();
        assert_eq!(Irq::new(72), Err(InterruptError::InvalidIrq));
    }
}
