//! AArch64 kernel cooperative-frame data, with no register capture or restore.

/// Rejected saved-context value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextError {
    /// Entry must be nonzero and aligned to a four-byte AArch64 instruction.
    InvalidEntry,
    /// Stack top must be nonzero and sixteen-byte aligned.
    InvalidStack,
    /// Only callee-saved registers x19 through x30 are represented.
    InvalidRegister,
}

/// C-layout, sixteen-byte-aligned cooperative kernel context representation.
///
/// Word offsets 0..12 contain x19..x30, followed by SP and PC. This does not
/// capture CPU registers, allocate/validate stack memory, or switch execution.
/// It is not an exception/EL0 frame: caller-saved registers, PSTATE, SIMD/FP,
/// address-space state, and a safe task-return trampoline are not implemented.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedContext {
    callee_saved: [u64; 12],
    stack_pointer: u64,
    program_counter: u64,
}

impl SavedContext {
    /// Construct initial frame data after checking entry/stack alignment only.
    /// The caller remains responsible for executable/mapped addresses and lifetime.
    pub fn for_kernel(entry: u64, stack_top: u64) -> Result<Self, ContextError> {
        if entry == 0 || entry % 4 != 0 {
            return Err(ContextError::InvalidEntry);
        }
        if stack_top == 0 || stack_top % 16 != 0 {
            return Err(ContextError::InvalidStack);
        }
        Ok(Self {
            callee_saved: [0; 12],
            stack_pointer: stack_top,
            program_counter: entry,
        })
    }

    /// Stored instruction address, not the live CPU program counter.
    pub const fn program_counter(&self) -> u64 {
        self.program_counter
    }

    /// Stored stack top, not the live CPU stack pointer.
    pub const fn stack_pointer(&self) -> u64 {
        self.stack_pointer
    }

    /// Read a represented register by its architectural number, x19..x30.
    pub fn register(&self, register: usize) -> Option<u64> {
        self.callee_saved.get(register.checked_sub(19)?).copied()
    }

    /// Update one represented register, rejecting out-of-range indices atomically.
    pub fn set_register(&mut self, register: usize, value: u64) -> Result<(), ContextError> {
        let offset = register
            .checked_sub(19)
            .ok_or(ContextError::InvalidRegister)?;
        let slot = self
            .callee_saved
            .get_mut(offset)
            .ok_or(ContextError::InvalidRegister)?;
        *slot = value;
        Ok(())
    }
}
