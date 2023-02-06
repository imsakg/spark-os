//! Fixed-capacity, statically linked kernel module lifecycle management.

use core::cell::Cell;

/// A module-defined initialization failure code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModuleFault(pub u16);

/// Statically linked callbacks and their named prerequisites.
#[derive(Clone, Copy)]
pub struct Descriptor {
    /// Unique, nonempty module name.
    pub name: &'static str,
    /// Names that must be active before `start` runs.
    pub dependencies: &'static [&'static str],
    /// Initialize; on error the callback must undo its own partial work.
    pub start: fn() -> Result<(), ModuleFault>,
    /// Infallibly release resources after successful initialization.
    pub stop: fn(),
}

/// Observable module lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Registered but not running.
    Registered,
    /// Initialization completed successfully.
    Active,
    /// The module's initialization callback failed.
    Failed,
}

/// Registry or lifecycle rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Empty module name.
    InvalidName,
    /// Another descriptor has this name.
    Duplicate,
    /// All descriptor slots are occupied.
    Full,
}

struct Entry {
    descriptor: Descriptor,
    state: Cell<State>,
}

/// Bounded module registry; no code loading, allocation, or interrupt synchronization.
pub struct Registry<const N: usize> {
    entries: [Option<Entry>; N],
}

impl<const N: usize> Registry<N> {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| None),
        }
    }

    /// Insert a descriptor without invoking any callbacks.
    pub fn register(&mut self, descriptor: Descriptor) -> Result<(), Error> {
        if descriptor.name.is_empty() {
            return Err(Error::InvalidName);
        }
        if self.find(descriptor.name).is_some() {
            return Err(Error::Duplicate);
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(Error::Full)?;
        *slot = Some(Entry {
            descriptor,
            state: Cell::new(State::Registered),
        });
        Ok(())
    }

    fn find(&self, name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.descriptor.name == name)
    }

    /// Number of registered descriptors, whether active or inactive.
    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    /// Whether no descriptors have been registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Look up immutable, statically linked metadata.
    pub fn descriptor(&self, name: &str) -> Option<&Descriptor> {
        self.find(name).map(|entry| &entry.descriptor)
    }

    /// Look up the current lifecycle state.
    pub fn state(&self, name: &str) -> Option<State> {
        self.find(name).map(|entry| entry.state.get())
    }
}

impl<const N: usize> Default for Registry<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    fn start() -> Result<(), ModuleFault> {
        Ok(())
    }
    fn stop() {}
    fn descriptor(name: &'static str) -> Descriptor {
        Descriptor {
            name,
            dependencies: &[],
            start,
            stop,
        }
    }
    #[test]
    fn registration_preserves_descriptor_and_initial_state() {
        let mut registry = Registry::<2>::new();
        assert_eq!(registry.register(descriptor("uart")), Ok(()));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.state("uart"), Some(State::Registered));
        assert_eq!(registry.descriptor("uart").unwrap().name, "uart");
        assert!(registry.descriptor("absent").is_none());
    }
    #[test]
    fn registration_rejects_invalid_duplicate_and_capacity() {
        let mut registry = Registry::<1>::new();
        assert_eq!(registry.register(descriptor("")), Err(Error::InvalidName));
        registry.register(descriptor("uart")).unwrap();
        assert_eq!(registry.register(descriptor("uart")), Err(Error::Duplicate));
        assert_eq!(registry.register(descriptor("gpio")), Err(Error::Full));
        assert_eq!(
            Registry::<0>::new().register(descriptor("x")),
            Err(Error::Full)
        );
    }
}
