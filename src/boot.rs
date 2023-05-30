//! Explicit, allocation-free boot diagnostics with named module dependencies.

use crate::modules::{Descriptor, Error, ModuleFault, Registry, State};

/// Completed local diagnostic suites; not hardware qualification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    /// Number of successfully initialized check modules.
    pub suites: usize,
}

fn memory() -> Result<(), ModuleFault> {
    crate::memory::self_test().map_err(|_| ModuleFault(1))
}
fn modules() -> Result<(), ModuleFault> {
    crate::modules::self_test().map_err(|_| ModuleFault(2))
}
fn drivers() -> Result<(), ModuleFault> {
    crate::drivers::self_test().map_err(|_| ModuleFault(3))
}
fn processes() -> Result<(), ModuleFault> {
    crate::process::self_test().map_err(|_| ModuleFault(4))
}
fn syscalls() -> Result<(), ModuleFault> {
    crate::syscall::self_test().map_err(|_| ModuleFault(5))
}
fn stop() {}

/// Run the same memory, lifecycle, driver-logic, scheduling and ABI suites at boot.
///
/// Descriptors are registered in deliberately reverse dependency order. All state
/// is stack-local and callbacks acquire no persistent hardware resources. This
/// leaves interrupts, MMU enablement, EL0 vectors and CPU switching untouched.
pub fn run_diagnostics() -> Result<Report, Error> {
    let mut registry = Registry::<5>::new();
    for descriptor in [
        Descriptor {
            name: "syscall-checks",
            dependencies: &["process-checks", "driver-checks"],
            start: syscalls,
            stop,
        },
        Descriptor {
            name: "driver-checks",
            dependencies: &["module-checks"],
            start: drivers,
            stop,
        },
        Descriptor {
            name: "process-checks",
            dependencies: &["memory-checks"],
            start: processes,
            stop,
        },
        Descriptor {
            name: "memory-checks",
            dependencies: &[],
            start: memory,
            stop,
        },
        Descriptor {
            name: "module-checks",
            dependencies: &[],
            start: modules,
            stop,
        },
    ] {
        registry.register(descriptor)?;
    }
    registry.initialize()?;
    for descriptor in registry.descriptors() {
        if registry.state(descriptor.name) != Some(State::Active) {
            return Err(Error::NotActive);
        }
    }
    Ok(Report {
        suites: registry.len(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_diagnostic_suites_execute_repeatably_in_dependency_order() {
        assert_eq!(super::run_diagnostics(), Ok(super::Report { suites: 5 }));
        assert_eq!(super::run_diagnostics(), Ok(super::Report { suites: 5 }));
    }
}
