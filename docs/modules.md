# Static kernel modules

`src/modules.rs` provides an allocation-free registry for **statically linked**
modules. It does not load executable images, relocate code, or install kernel
extensions. `Registry<N>` owns at most `N` descriptors, using stack/inline storage.
`Descriptor` contains a unique nonempty name, named prerequisites, a fallible
`start` function, and an infallible `stop` function.

## Lifecycle contract

- `register` inserts metadata only: state is `Registered`; no callback runs.
- `initialize` first validates all dependency names and computes a bounded
  topological order. Missing dependencies and cycles produce no initialization
  side effects. Registration order breaks ties between independent modules.
- Dependencies start before consumers. Already `Active` modules are skipped.
- A failed start marks that module `Failed`; its callback must clean up its own
  partial work. Previously successful starts **from this call** are stopped in
  reverse initialization order and become `Registered`. Preexisting active
  modules remain active. A later explicit `initialize` retries inactive/failed
  modules; there is no implicit retry loop.
- `lease(name)` returns a counted `ModuleLease` only for an active module.
  `Drop` releases that usage count, including on normal early returns.
- `unload(name)` refuses live leases and active named consumers. Success calls
  `stop` once and retains metadata in `Registered` state; code remains linked.
  Callers must use leases for every externally retained resource reference.
- `unregister` removes only inactive, unused metadata with no remaining named
  consumers, including inactive consumers. Remove consumer metadata first.
  Slots are reused without stale name lookup. `descriptors` skips vacant slots.

The registry is deliberately `!Sync` through interior `Cell` state. Board code
must serialize it, device callbacks, and users against interrupts/other cores.
It is not an SMP lock, quiescent-state mechanism, or reclamation system. A leaked
lease conservatively keeps a module busy. Panicking callbacks are not recovered;
bare-metal panic policy applies. Dropping a registry does not call module stops:
explicitly unload consumers before their providers.

## Devices and modules

`drivers::manager::DeviceManager<N>` exclusively borrows concrete `Device`
implementations, snapshots `Metadata`, and initializes in registration order.
`Metadata::requires` can name an active registry module. The manager acquires all
new prerequisites before any initialization callback and keeps their leases
until the ready device is shut down. A module cannot be unloaded underneath a
registered ready device using this contract.

Device failure rolls back only this call's newly initialized devices, preserving
preexisting ready devices, and releases all new leases. Failed device callbacks
must undo their own partial work. Explicit `shutdown` and manager `Drop` stop
ready devices in reverse registration order; repeated shutdown is harmless.
The manager supplies lifecycle policy, not automatic hardware discovery or
board-specific transport/clock setup.

## Boot diagnostic and verification

`modules::self_test() -> Result<(), &'static str>` uses local registry state to
check dependency order/state, leases, unload guards, slot reuse, and failure
rollback. It allocates nothing, touches no globals/devices, and is repeatable.
Call it explicitly from boot diagnostics; it is not installed automatically.

Run the dependency-free host suite using the pinned Rust 1.66-era nightly:

```sh
sh tools/host-test.sh
rustc --edition=2021 --crate-type lib src/lib.rs -D warnings -D missing_docs \
  -o target/host/libspark_os.rlib
rustc --edition=2021 --crate-type lib --target aarch64-unknown-none-softfloat \
  src/lib.rs -D warnings -D missing_docs -o target/host/libspark_os-aarch64.rlib
```

Host tests exercise reverse registration, cycle/missing-name rejection,
rollback ordering, retry, capacity boundaries (including zero), lease overflow,
and dependent/busy unload rejection. They do not prove interrupt safety or
physical-device behavior.
