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
    /// No module has the requested name.
    NotFound,
    /// The module is not currently running.
    NotActive,
    /// A usage lease or active state prevents the operation.
    Busy,
    /// A running consumer still needs the module.
    DependentActive(&'static str),
    /// Another registered descriptor still names this prerequisite.
    ReferencedBy(&'static str),
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
    users: Cell<u32>,
}

/// A counted usage lease; dropping it releases one user of an active module.
///
/// Leases do not expose dynamically loaded code. The registry is single-core
/// serialized (`!Sync`); callers must not bypass this boundary from interrupts.
pub struct ModuleLease<'a> {
    entry: &'a Entry,
}

impl ModuleLease<'_> {
    /// Immutable metadata for the leased module.
    pub fn descriptor(&self) -> &Descriptor {
        &self.entry.descriptor
    }
}

impl Drop for ModuleLease<'_> {
    fn drop(&mut self) {
        self.entry.users.set(self.entry.users.get() - 1);
    }
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
            users: Cell::new(0),
        });
        Ok(())
    }

    /// Initialize inactive modules in dependency order.
    ///
    /// Missing names and cycles are checked before any callback runs. Active
    /// modules are not started again. A failure stops newly initialized modules
    /// in reverse order, preserving preexisting active modules. Failed modules
    /// may be retried explicitly by calling this method again.
    pub fn initialize(&mut self) -> Result<(), Error> {
        let (order, count) = self.plan()?;
        for (completed, &index) in order[..count].iter().enumerate() {
            let entry = self.entries[index].as_ref().unwrap();
            if let Err(fault) = (entry.descriptor.start)() {
                entry.state.set(State::Failed);
                for &previous in order[..completed].iter().rev() {
                    let started = self.entries[previous].as_ref().unwrap();
                    (started.descriptor.stop)();
                    started.state.set(State::Registered);
                }
                return Err(Error::Initialization {
                    module: entry.descriptor.name,
                    fault,
                });
            }
            entry.state.set(State::Active);
        }
        Ok(())
    }

    /// Hold a running module in use until the returned lease is dropped.
    pub fn lease(&self, name: &str) -> Result<ModuleLease<'_>, Error> {
        let entry = self.find(name).ok_or(Error::NotFound)?;
        if entry.state.get() != State::Active {
            return Err(Error::NotActive);
        }
        let users = entry.users.get().checked_add(1).ok_or(Error::Busy)?;
        entry.users.set(users);
        Ok(ModuleLease { entry })
    }

    /// Stop an unleased module with no active consumers; retain its descriptor.
    pub fn unload(&self, name: &str) -> Result<(), Error> {
        let entry = self.find(name).ok_or(Error::NotFound)?;
        if entry.state.get() != State::Active {
            return Err(Error::NotActive);
        }
        if entry.users.get() != 0 {
            return Err(Error::Busy);
        }
        for consumer in self.entries.iter().flatten() {
            if consumer.state.get() == State::Active
                && consumer.descriptor.dependencies.contains(&name)
            {
                return Err(Error::DependentActive(consumer.descriptor.name));
            }
        }
        (entry.descriptor.stop)();
        entry.state.set(State::Registered);
        Ok(())
    }

    /// Iterate metadata in slot order, skipping vacant slots.
    pub fn descriptors(&self) -> impl Iterator<Item = &Descriptor> {
        self.entries.iter().flatten().map(|entry| &entry.descriptor)
    }

    /// Remove inactive metadata only after all consumers have been removed.
    pub fn unregister(&mut self, name: &str) -> Result<(), Error> {
        let index = self
            .entries
            .iter()
            .position(|entry| {
                entry
                    .as_ref()
                    .map_or(false, |entry| entry.descriptor.name == name)
            })
            .ok_or(Error::NotFound)?;
        let entry = self.entries[index].as_ref().unwrap();
        if entry.state.get() == State::Active || entry.users.get() != 0 {
            return Err(Error::Busy);
        }
        for consumer in self.entries.iter().flatten() {
            if consumer.descriptor.dependencies.contains(&name) {
                return Err(Error::ReferencedBy(consumer.descriptor.name));
            }
        }
        self.entries[index] = None;
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

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use std::sync::Mutex;
    static EVENTS: Mutex<std::vec::Vec<u8>> = Mutex::new(std::vec::Vec::new());
    fn start_a() -> Result<(), ModuleFault> {
        EVENTS.lock().unwrap().push(1);
        Ok(())
    }
    fn start_b() -> Result<(), ModuleFault> {
        EVENTS.lock().unwrap().push(2);
        Ok(())
    }
    fn fail() -> Result<(), ModuleFault> {
        EVENTS.lock().unwrap().push(3);
        Err(ModuleFault(42))
    }
    fn stop_a() {
        EVENTS.lock().unwrap().push(5);
    }
    fn stop_b() {
        EVENTS.lock().unwrap().push(4);
    }
    fn unused() {
        panic!("failed initializer must clean itself; stop is not called");
    }
    #[test]
    fn failure_rolls_back_only_newly_started_modules_in_reverse_order() {
        EVENTS.lock().unwrap().clear();
        let mut registry = Registry::<4>::new();
        registry
            .register(Descriptor {
                name: "base",
                dependencies: &[],
                start: start_a,
                stop: unused,
            })
            .unwrap();
        registry.initialize().unwrap();
        EVENTS.lock().unwrap().clear();
        registry
            .register(Descriptor {
                name: "a",
                dependencies: &["base"],
                start: start_a,
                stop: stop_a,
            })
            .unwrap();
        registry
            .register(Descriptor {
                name: "b",
                dependencies: &["a"],
                start: start_b,
                stop: stop_b,
            })
            .unwrap();
        registry
            .register(Descriptor {
                name: "bad",
                dependencies: &["b"],
                start: fail,
                stop: unused,
            })
            .unwrap();
        assert_eq!(
            registry.initialize(),
            Err(Error::Initialization {
                module: "bad",
                fault: ModuleFault(42)
            })
        );
        assert_eq!(*EVENTS.lock().unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(registry.state("base"), Some(State::Active));
        assert_eq!(registry.state("a"), Some(State::Registered));
        assert_eq!(registry.state("b"), Some(State::Registered));
        assert_eq!(registry.state("bad"), Some(State::Failed));
    }
}

#[cfg(test)]
mod unload_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STOPS: AtomicUsize = AtomicUsize::new(0);
    fn start() -> Result<(), ModuleFault> {
        Ok(())
    }
    fn stop() {
        STOPS.fetch_add(1, Ordering::SeqCst);
    }
    #[test]
    fn leases_and_active_dependents_block_unload() {
        STOPS.store(0, Ordering::SeqCst);
        let mut registry = Registry::<2>::new();
        registry
            .register(Descriptor {
                name: "uart",
                dependencies: &[],
                start,
                stop,
            })
            .unwrap();
        registry
            .register(Descriptor {
                name: "console",
                dependencies: &["uart"],
                start,
                stop,
            })
            .unwrap();
        assert!(matches!(registry.lease("uart"), Err(Error::NotActive)));
        registry.initialize().unwrap();
        assert_eq!(
            registry.unload("uart"),
            Err(Error::DependentActive("console"))
        );
        let lease = registry.lease("console").unwrap();
        assert_eq!(lease.descriptor().name, "console");
        assert_eq!(registry.unload("console"), Err(Error::Busy));
        assert_eq!(STOPS.load(Ordering::SeqCst), 0);
        drop(lease);
        registry.unload("console").unwrap();
        registry.unload("uart").unwrap();
        assert_eq!(STOPS.load(Ordering::SeqCst), 2);
        assert_eq!(registry.unload("uart"), Err(Error::NotActive));
        assert_eq!(registry.unload("absent"), Err(Error::NotFound));
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;
    fn start() -> Result<(), ModuleFault> {
        Ok(())
    }
    fn stop() {}
    fn descriptor(name: &'static str, dependencies: &'static [&'static str]) -> Descriptor {
        Descriptor {
            name,
            dependencies,
            start,
            stop,
        }
    }
    #[test]
    fn removal_reuses_slots_without_stale_lookup_or_dangling_dependencies() {
        let mut registry = Registry::<2>::new();
        registry.register(descriptor("a", &[])).unwrap();
        registry.register(descriptor("b", &["a"])).unwrap();
        assert_eq!(registry.unregister("a"), Err(Error::ReferencedBy("b")));
        assert_eq!(registry.unregister("missing"), Err(Error::NotFound));
        registry.unregister("b").unwrap();
        assert_eq!(registry.state("b"), None);
        registry.register(descriptor("c", &[])).unwrap();
        let names: std::vec::Vec<_> = registry.descriptors().map(|d| d.name).collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(registry.len(), 2);
        registry.initialize().unwrap();
        assert_eq!(registry.unregister("c"), Err(Error::Busy));
        registry.unload("c").unwrap();
        registry.unregister("c").unwrap();
        registry.unload("a").unwrap();
        registry.unregister("a").unwrap();
        assert!(registry.is_empty());
        assert_eq!(registry.descriptors().count(), 0);
    }
    #[test]
    fn usage_counter_overflow_is_rejected_without_wrapping() {
        let mut registry = Registry::<1>::new();
        registry.register(descriptor("a", &[])).unwrap();
        registry.initialize().unwrap();
        let entry = registry.find("a").unwrap();
        entry.users.set(u32::MAX);
        assert!(matches!(registry.lease("a"), Err(Error::Busy)));
        assert_eq!(entry.users.get(), u32::MAX);
    }
}
