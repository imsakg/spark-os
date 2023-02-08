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
    /// A named prerequisite has not been registered.
    MissingDependency(&'static str),
    /// The dependency graph contains a cycle.
    DependencyCycle,
    /// An initialization hook failed.
    Initialization {
        /// Name of the failing module.
        module: &'static str,
        /// Failure code supplied by the hook.
        fault: ModuleFault,
    },
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

    /// Initialize inactive modules in dependency order.
    ///
    /// Missing names and cycles are checked before any callback runs. Active
    /// modules are not started again. Failed modules may be retried explicitly
    /// by calling this method again.
    pub fn initialize(&mut self) -> Result<(), Error> {
        let (order, count) = self.plan()?;
        for &index in &order[..count] {
            let entry = self.entries[index].as_ref().unwrap();
            if let Err(fault) = (entry.descriptor.start)() {
                entry.state.set(State::Failed);
                return Err(Error::Initialization {
                    module: entry.descriptor.name,
                    fault,
                });
            }
            entry.state.set(State::Active);
        }
        Ok(())
    }

    fn plan(&self) -> Result<([usize; N], usize), Error> {
        for entry in self.entries.iter().flatten() {
            for &dependency in entry.descriptor.dependencies {
                if self.find(dependency).is_none() {
                    return Err(Error::MissingDependency(dependency));
                }
            }
        }
        let mut selected = [false; N];
        let mut remaining = 0;
        for (index, slot) in self.entries.iter().enumerate() {
            selected[index] = slot
                .as_ref()
                .map_or(true, |e| e.state.get() == State::Active);
            if !selected[index] {
                remaining += 1;
            }
        }
        let mut order = [0; N];
        let mut count = 0;
        while remaining != 0 {
            let before = remaining;
            for (index, slot) in self.entries.iter().enumerate() {
                if selected[index] {
                    continue;
                }
                let entry = slot.as_ref().unwrap();
                let ready = entry.descriptor.dependencies.iter().all(|dependency| {
                    self.entries.iter().enumerate().any(|(i, e)| {
                        selected[i]
                            && e.as_ref()
                                .map_or(false, |e| e.descriptor.name == *dependency)
                    })
                });
                if ready {
                    selected[index] = true;
                    order[count] = index;
                    count += 1;
                    remaining -= 1;
                }
            }
            if remaining == before {
                return Err(Error::DependencyCycle);
            }
        }
        Ok((order, count))
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

#[cfg(test)]
mod dependency_tests {
    use super::*;
    use std::sync::Mutex;
    static CALLS: Mutex<std::vec::Vec<u8>> = Mutex::new(std::vec::Vec::new());
    fn first() -> Result<(), ModuleFault> {
        CALLS.lock().unwrap().push(1);
        Ok(())
    }
    fn second() -> Result<(), ModuleFault> {
        CALLS.lock().unwrap().push(2);
        Ok(())
    }
    fn stop() {}
    #[test]
    fn dependencies_start_before_reverse_registered_consumers() {
        CALLS.lock().unwrap().clear();
        let mut registry = Registry::<3>::new();
        registry
            .register(Descriptor {
                name: "console",
                dependencies: &["uart"],
                start: second,
                stop,
            })
            .unwrap();
        registry
            .register(Descriptor {
                name: "uart",
                dependencies: &[],
                start: first,
                stop,
            })
            .unwrap();
        assert_eq!(registry.initialize(), Ok(()));
        assert_eq!(*CALLS.lock().unwrap(), [1, 2]);
        assert_eq!(registry.state("console"), Some(State::Active));
        registry.initialize().unwrap();
        assert_eq!(*CALLS.lock().unwrap(), [1, 2]);
    }
    fn ok() -> Result<(), ModuleFault> {
        Ok(())
    }
    #[test]
    fn missing_and_cyclic_graphs_are_rejected_before_start() {
        let mut registry = Registry::<2>::new();
        registry
            .register(Descriptor {
                name: "x",
                dependencies: &["y"],
                start: ok,
                stop,
            })
            .unwrap();
        assert_eq!(registry.initialize(), Err(Error::MissingDependency("y")));
        assert_eq!(registry.state("x"), Some(State::Registered));
        registry
            .register(Descriptor {
                name: "y",
                dependencies: &["x"],
                start: ok,
                stop,
            })
            .unwrap();
        assert_eq!(registry.initialize(), Err(Error::DependencyCycle));
        assert_eq!(registry.state("x"), Some(State::Registered));
        assert_eq!(registry.state("y"), Some(State::Registered));
    }
}
