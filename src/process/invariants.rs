//! Allocation-free scheduler consistency diagnostics.

/// A structural scheduler invariant was violated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvariantError {
    /// A retained identity is duplicated, reserved, or ahead of the PID source.
    Identity,
    /// Ready metadata and duplicate-free queue membership disagree.
    ReadyQueue,
    /// Running process metadata does not match the unique current identity.
    Current,
    /// Slice configuration, current/idle budget, or remaining budget is invalid.
    Quantum,
    /// Wait reasons/deadlines do not match lifecycle state.
    WaitMetadata,
    /// Exit status is present exactly when the process is a zombie.
    ExitMetadata,
    /// Parent/child linkage is missing, terminal, cyclic, or inconsistent with a wait.
    Parent,
}
