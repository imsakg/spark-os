//! Spark kernel call ABI. Dispatch is explicit; no EL0 exception vector is installed.

mod user;
pub use user::UserMemory;

/// Stable numeric errors, encoded as negative signed return values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum Errno {
    /// Caller has no selected process.
    NoCurrent = 3,
    /// Device rejected an operation.
    Io = 5,
    /// File descriptor is not valid for the operation.
    BadFd = 9,
    /// The requested process is not a child.
    NoChild = 10,
    /// A nonblocking operation cannot currently progress.
    Again = 11,
    /// User-buffer address or length is outside admitted storage.
    Fault = 14,
    /// Invalid argument or state transition.
    Invalid = 22,
    /// No pipe reader remains.
    Pipe = 32,
    /// Unsupported call number.
    NoSys = 38,
    /// A value cannot be represented by the ABI.
    Overflow = 75,
}

/// Kernel service result before machine-register encoding.
pub type CallResult = Result<u64, Errno>;

/// Allocation-free call request decoded from the AArch64 register convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    /// Call number from x8.
    pub number: u64,
    /// Arguments from x0 through x5.
    pub arguments: [u64; 6],
}

/// Saved general registers supplied by a future exception-vector entry.
/// This type does not capture or restore registers itself.
#[repr(C)]
pub struct TrapFrame {
    /// x0 through x30; x8 is the call number and x0 is the return register.
    pub registers: [u64; 31],
}

impl TrapFrame {
    /// Decode arguments without changing the saved register image.
    pub fn request(&self) -> Request {
        let mut arguments = [0; 6];
        arguments.copy_from_slice(&self.registers[..6]);
        Request {
            number: self.registers[8],
            arguments,
        }
    }

    /// Encode a result while preserving every register except x0.
    pub fn complete(&mut self, result: CallResult) {
        let signed = match result {
            Ok(value) => i64::try_from(value).unwrap_or(-(Errno::Overflow as i64)),
            Err(error) => -(error as i64),
        };
        self.registers[0] = signed as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_uses_x8_and_x0_through_x5_without_touching_saved_registers() {
        let mut frame = TrapFrame {
            registers: [0xfeed; 31],
        };
        frame.registers[8] = 172;
        frame.registers[..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            frame.request(),
            Request {
                number: 172,
                arguments: [1, 2, 3, 4, 5, 6]
            }
        );
        frame.complete(Err(Errno::NoSys));
        assert_eq!(frame.registers[0] as i64, -38);
        assert_eq!(frame.registers[19], 0xfeed);
        frame.complete(Ok(u64::MAX));
        assert_eq!(frame.registers[0] as i64, -75);
    }
}
