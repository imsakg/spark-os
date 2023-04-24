//! Bounded device lifecycle management with module usage leases.

use crate::modules::{self, ModuleLease, Registry};

/// Device role, used for discovery rather than hardware capability claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceKind {
    /// Console/serial byte device.
    Console,
    /// General-purpose pin controller.
    Gpio,
    /// Clock or alarm source.
    Timer,
    /// Interrupt routing controller.
    InterruptController,
    /// Firmware mailbox endpoint.
    Mailbox,
    /// A board-specific device outside the common roles.
    Other,
}

/// Stable device registration metadata, snapshotted before initialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    /// Nonempty unique device name.
    pub name: &'static str,
    /// Broad device role.
    pub kind: DeviceKind,
    /// Optional module to keep leased throughout the device's ready lifetime.
    pub requires: Option<&'static str>,
}

/// Device-specific initialization failure code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceFault(pub u16);

/// Serialized device lifecycle; implementations retain their concrete transport.
pub trait Device {
    /// Return immutable registration identity and dependency metadata.
    fn metadata(&self) -> Metadata;
    /// Initialize hardware; on failure undo this callback's own partial work.
    fn initialize(&mut self) -> Result<(), DeviceFault>;
    /// Infallibly quiesce/release resources after successful initialization.
    fn shutdown(&mut self);
}

/// Device lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceState {
    /// Registered but not initialized.
    Registered,
    /// Initialization succeeded; any required module lease remains held.
    Ready,
    /// The device's initialization failed.
    Failed,
}

/// Registration or device initialization rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceError {
    /// Device name was empty.
    InvalidName,
    /// A registered device already has this name.
    Duplicate,
    /// All device slots are occupied.
    Full,
    /// A required module could not be leased before callbacks began.
    Dependency {
        /// Device whose prerequisite could not be acquired.
        device: &'static str,
        /// Exact module-registry rejection.
        reason: modules::Error,
    },
    /// A device initialization callback failed.
    Initialization {
        /// Failing device name.
        device: &'static str,
        /// Device-specific failure code.
        fault: DeviceFault,
    },
}

struct Managed<'d, 'm> {
    device: &'d mut dyn Device,
    metadata: Metadata,
    state: DeviceState,
    lease: Option<ModuleLease<'m>>,
}

/// Fixed-capacity borrowed-device manager, with no allocation or global registry.
/// Drop shuts ready devices down in reverse registration order.
pub struct DeviceManager<'d, 'm, const N: usize> {
    entries: [Option<Managed<'d, 'm>>; N],
}
impl<'d, 'm, const N: usize> DeviceManager<'d, 'm, N> {
    /// Construct an empty manager.
    pub fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| None),
        }
    }
    /// Borrow a device exclusively until the manager is dropped.
    pub fn register(&mut self, device: &'d mut dyn Device) -> Result<(), DeviceError> {
        let metadata = device.metadata();
        if metadata.name.is_empty() {
            return Err(DeviceError::InvalidName);
        }
        if self.metadata(metadata.name).is_some() {
            return Err(DeviceError::Duplicate);
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|e| e.is_none())
            .ok_or(DeviceError::Full)?;
        *slot = Some(Managed {
            device,
            metadata,
            state: DeviceState::Registered,
            lease: None,
        });
        Ok(())
    }
    /// Look up the snapshotted metadata by exact device name.
    pub fn metadata(&self, name: &str) -> Option<&Metadata> {
        self.entries
            .iter()
            .flatten()
            .find(|e| e.metadata.name == name)
            .map(|e| &e.metadata)
    }
    /// Look up a device's current lifecycle state.
    pub fn state(&self, name: &str) -> Option<DeviceState> {
        self.entries
            .iter()
            .flatten()
            .find(|e| e.metadata.name == name)
            .map(|e| e.state)
    }
    /// Acquire all prerequisites before callbacks; initialize in registration order.
    /// Failure rolls back only this call's successful devices, in reverse order,
    /// and releases all newly acquired leases. Existing ready devices are kept.
    pub fn initialize<const M: usize>(
        &mut self,
        modules: &'m Registry<M>,
    ) -> Result<(), DeviceError> {
        let mut order = [0; N];
        let mut count = 0;
        for index in 0..N {
            let metadata = match self.entries[index].as_ref() {
                Some(entry) if entry.state != DeviceState::Ready => entry.metadata,
                _ => continue,
            };
            let lease = if let Some(required) = metadata.requires {
                match modules.lease(required) {
                    Ok(lease) => Some(lease),
                    Err(reason) => {
                        for &previous in &order[..count] {
                            self.entries[previous].as_mut().unwrap().lease = None;
                        }
                        return Err(DeviceError::Dependency {
                            device: metadata.name,
                            reason,
                        });
                    }
                }
            } else {
                None
            };
            self.entries[index].as_mut().unwrap().lease = lease;
            order[count] = index;
            count += 1;
        }
        for (completed, &index) in order[..count].iter().enumerate() {
            let entry = self.entries[index].as_mut().unwrap();
            if let Err(fault) = entry.device.initialize() {
                entry.state = DeviceState::Failed;
                let device = entry.metadata.name;
                for &unstarted in &order[completed..count] {
                    self.entries[unstarted].as_mut().unwrap().lease = None;
                }
                for &previous in order[..completed].iter().rev() {
                    let started = self.entries[previous].as_mut().unwrap();
                    started.device.shutdown();
                    started.state = DeviceState::Registered;
                    started.lease = None;
                }
                return Err(DeviceError::Initialization { device, fault });
            }
            entry.state = DeviceState::Ready;
        }
        Ok(())
    }
    /// Stop ready devices in reverse order and release their module leases.
    /// Repeated shutdown is a no-op; failed callbacks are not shut down twice.
    pub fn shutdown(&mut self) {
        for entry in self.entries.iter_mut().rev().flatten() {
            if entry.state == DeviceState::Ready {
                entry.device.shutdown();
                entry.state = DeviceState::Registered;
                entry.lease = None;
            }
        }
    }
}
impl<const N: usize> Default for DeviceManager<'_, '_, N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Drop for DeviceManager<'_, '_, N> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{Descriptor, ModuleFault, Registry};
    use std::cell::RefCell;
    struct TestDevice<'a> {
        metadata: Metadata,
        id: u8,
        fail: bool,
        events: &'a RefCell<std::vec::Vec<u8>>,
    }
    impl Device for TestDevice<'_> {
        fn metadata(&self) -> Metadata {
            self.metadata
        }
        fn initialize(&mut self) -> Result<(), DeviceFault> {
            self.events.borrow_mut().push(self.id);
            if self.fail {
                Err(DeviceFault(7))
            } else {
                Ok(())
            }
        }
        fn shutdown(&mut self) {
            self.events.borrow_mut().push(self.id + 10);
        }
    }
    fn start() -> Result<(), ModuleFault> {
        Ok(())
    }
    fn stop() {}
    fn modules() -> Registry<1> {
        let mut registry = Registry::new();
        registry
            .register(Descriptor {
                name: "bus",
                dependencies: &[],
                start,
                stop,
            })
            .unwrap();
        registry.initialize().unwrap();
        registry
    }
    fn metadata(name: &'static str) -> Metadata {
        Metadata {
            name,
            kind: DeviceKind::Console,
            requires: Some("bus"),
        }
    }
    #[test]
    fn ready_device_keeps_module_busy_until_shutdown_and_drop_is_idempotent() {
        let modules = modules();
        let events = RefCell::new(std::vec::Vec::new());
        let mut device = TestDevice {
            metadata: metadata("uart"),
            id: 1,
            fail: false,
            events: &events,
        };
        {
            let mut manager = DeviceManager::<1>::new();
            manager.register(&mut device).unwrap();
            manager.initialize(&modules).unwrap();
            manager.initialize(&modules).unwrap();
            assert_eq!(manager.state("uart"), Some(DeviceState::Ready));
            assert_eq!(manager.metadata("uart").unwrap().kind, DeviceKind::Console);
            assert_eq!(modules.unload("bus"), Err(crate::modules::Error::Busy));
            manager.shutdown();
            modules.unload("bus").unwrap();
        }
        assert_eq!(*events.borrow(), [1, 11]);
    }
    #[test]
    fn device_failure_rolls_back_and_releases_every_new_module_lease() {
        let modules = modules();
        let events = RefCell::new(std::vec::Vec::new());
        let mut first = TestDevice {
            metadata: metadata("uart"),
            id: 1,
            fail: false,
            events: &events,
        };
        let mut bad = TestDevice {
            metadata: metadata("bad"),
            id: 2,
            fail: true,
            events: &events,
        };
        let mut manager = DeviceManager::<2>::new();
        manager.register(&mut first).unwrap();
        manager.register(&mut bad).unwrap();
        assert_eq!(
            manager.initialize(&modules),
            Err(DeviceError::Initialization {
                device: "bad",
                fault: DeviceFault(7)
            })
        );
        assert_eq!(*events.borrow(), [1, 2, 11]);
        assert_eq!(manager.state("uart"), Some(DeviceState::Registered));
        assert_eq!(manager.state("bad"), Some(DeviceState::Failed));
        modules.unload("bus").unwrap();
    }
    #[test]
    fn partial_dependency_preflight_releases_earlier_acquisitions() {
        let modules = modules();
        let events = RefCell::new(std::vec::Vec::new());
        let mut first = TestDevice {
            metadata: metadata("uart"),
            id: 1,
            fail: false,
            events: &events,
        };
        let mut missing = TestDevice {
            metadata: Metadata {
                requires: Some("missing"),
                ..metadata("second")
            },
            id: 2,
            fail: false,
            events: &events,
        };
        let mut manager = DeviceManager::<2>::new();
        manager.register(&mut first).unwrap();
        manager.register(&mut missing).unwrap();
        assert!(matches!(
            manager.initialize(&modules),
            Err(DeviceError::Dependency {
                device: "second",
                ..
            })
        ));
        assert!(events.borrow().is_empty());
        modules.unload("bus").unwrap();
    }
    #[test]
    fn later_failure_preserves_ready_devices_and_drop_releases_their_leases() {
        let modules = modules();
        let events = RefCell::new(std::vec::Vec::new());
        let mut ready = TestDevice {
            metadata: metadata("ready"),
            id: 1,
            fail: false,
            events: &events,
        };
        let mut next = TestDevice {
            metadata: metadata("next"),
            id: 2,
            fail: false,
            events: &events,
        };
        let mut bad = TestDevice {
            metadata: metadata("bad"),
            id: 3,
            fail: true,
            events: &events,
        };
        {
            let mut manager = DeviceManager::<3>::new();
            manager.register(&mut ready).unwrap();
            manager.initialize(&modules).unwrap();
            manager.register(&mut next).unwrap();
            manager.register(&mut bad).unwrap();
            assert!(manager.initialize(&modules).is_err());
            assert_eq!(*events.borrow(), [1, 2, 3, 12]);
            assert_eq!(manager.state("ready"), Some(DeviceState::Ready));
            assert_eq!(modules.unload("bus"), Err(crate::modules::Error::Busy));
        }
        assert_eq!(*events.borrow(), [1, 2, 3, 12, 11]);
        modules.unload("bus").unwrap();
    }
    #[test]
    fn dependency_preflight_prevents_callbacks_and_metadata_capacity_is_bounded() {
        let modules = Registry::<0>::new();
        let events = RefCell::new(std::vec::Vec::new());
        let mut first = TestDevice {
            metadata: metadata("uart"),
            id: 1,
            fail: false,
            events: &events,
        };
        let mut duplicate = TestDevice {
            metadata: metadata("uart"),
            id: 2,
            fail: false,
            events: &events,
        };
        let mut extra = TestDevice {
            metadata: metadata("extra"),
            id: 3,
            fail: false,
            events: &events,
        };
        let mut manager = DeviceManager::<1>::new();
        manager.register(&mut first).unwrap();
        assert_eq!(
            manager.register(&mut duplicate),
            Err(DeviceError::Duplicate)
        );
        assert_eq!(manager.register(&mut extra), Err(DeviceError::Full));
        assert_eq!(
            manager.initialize(&modules),
            Err(DeviceError::Dependency {
                device: "uart",
                reason: crate::modules::Error::NotFound
            })
        );
        assert!(events.borrow().is_empty());
    }
}
