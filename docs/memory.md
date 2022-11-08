# Memory services

The `spark_os::memory` library provides allocation-free memory bookkeeping for
4 KiB pages and frames. It is `no_std`, has no registry dependencies, and builds
with the repository's Rust 1.66 nightly (`nightly-2022-10-13`). Production memory
code uses safe Rust; the host reference-model tests may use `std` collections.

**These are software services, not an enabled hardware MMU.** Address wrappers,
frame allocations, region records, and mapping plans never dereference their
physical or virtual addresses. Descriptors encode bits only. Byte arenas and byte
helpers access only initialized slices supplied by their caller.

This guide is included in the module documentation. All three Rust examples below
are compiled and executed by `rustdoc --test`; none is an ignored sketch.

## Boot-time self-test integration

The public runtime entry point is:

`memory::self_test() -> Result<(), memory::MemoryError>`

It uses local stack-backed bitmaps, region and mapping tables, and an initialized
byte arena. It exercises reservations, aligned cross-bitmap-word allocation,
accounting, translation and permission checks, incompatible physical aliases,
protection changes, descriptor encoding, explicit unmap/release, overlapping byte
moves, and checkpoint scrubbing. It has no persistent state and is repeatable.
Unexpected invariant results return `SelfTestFailed`; operation errors propagate.
It needs a usable Rust stack, but neither a heap nor an enabled MMU. It does not
probe real RAM, devices, translation registers, cache coherency, or TLB behavior.

The current `src/main.rs` does **not** declare `memory` or invoke this check.
To opt in, add `mod memory;` beside the binary's other module declarations and call
the following check during the existing single-core `kernel_init`, before handing
memory state to other cores. The library and binary then compile the same source
module. Choose the boot failure policy explicitly; this example uses the existing
kernel panic path rather than silently continuing after a failure:

```rust
# use spark_os::memory;
if let Err(error) = memory::self_test() {
    panic!("memory self-test failed: {:?}", error);
}
```

On a host, use `spark_os::memory` as above. There is no separate `init_mmu` API or
implicit boot hook, and this change does not wire the check into the kernel binary.

## A complete software boot-memory plan

The following executable example constructs a known usable RAM region, reserves
its boot-owned prefix, allocates an aligned extent, maps and protects it, encodes
one leaf descriptor, and explicitly releases the frames after unmapping. The
addresses are sample bookkeeping values, **not** a board memory map. Actual boot
code must derive RAM and exclusions from the BSP/firmware, retain caller-owned
bitmaps and tables for their required lifetimes, and supply synchronization.

```rust
use spark_os::memory::{
    AccessType, BitmapFrames, CachePolicy, Mapping, MappingTable, MemoryError,
    MemoryKind, MemoryRegion, PageDescriptor, PagePermissions, PageRange,
    PhysAddr, RegionAttributes, RegionMap, VirtAddr, PAGE_SIZE,
};

fn main() -> Result<(), MemoryError> {
    let attributes = RegionAttributes::new(MemoryKind::Usable, CachePolicy::WriteBack)?;
    let ram = MemoryRegion::new(PhysAddr::new(0x10_0000), 128 * PAGE_SIZE, attributes)?;
    let mut regions = RegionMap::<2>::new();
    regions.insert(ram)?;
    assert_eq!(regions.usable_bytes(), ram.byte_len());

    // ceil(128 / 64) words in each of two separate caller-owned bitmaps.
    let mut used = [0u64; 2];
    let mut reserved = [0u64; 2];
    let mut frames = BitmapFrames::new(ram.start(), 128, &mut used, &mut reserved)?;
    // Real exclusions include kernel image, stacks, boot data, bitmaps, and tables.
    frames.reserve(ram.start(), 16)?;
    let extent = frames.allocate_aligned(4, 4 * PAGE_SIZE)?;
    assert_eq!(extent.start().get() % (4 * PAGE_SIZE), 0);
    assert_eq!(regions.lookup(extent.start()), Some(ram));
    assert!(extent.end() <= ram.end());

    let range = PageRange::new(VirtAddr::new(0x4000_0000), extent.frame_count())?;
    let data = PagePermissions::new(true, false, false)?;
    let mapping = Mapping::new(range, extent.start(), data, attributes.cache())?;
    let mut plan = MappingTable::<4>::new();
    plan.map(mapping)?;
    let byte = range.start().checked_add(PAGE_SIZE + 17)?;
    assert_eq!(plan.resolve(byte, AccessType::Write, false)?,
               extent.start().checked_add(PAGE_SIZE + 17)?);

    // Offline software transition only; no hardware execution is attempted.
    let code = PagePermissions::new(false, true, false)?;
    assert_eq!(plan.protect(range, code)?, mapping); // Returns previous mapping.
    assert_eq!(plan.resolve(byte, AccessType::Write, false),
               Err(MemoryError::PermissionDenied));
    let leaf = PageDescriptor::new(extent.start(), code, attributes.cache())?;
    assert_eq!(leaf.output_address(), extent.start());
    assert_eq!(leaf.bits() & 3, 3);

    let removed = plan.unmap(range)?;
    assert_eq!(removed.physical_start(), extent.start());
    assert!(plan.is_empty());
    assert_eq!(frames.stats().allocated, 4); // Unmapping never frees frames.
    frames.free_extent(extent)?;
    assert_eq!(frames.stats().allocated, 0);
    assert_eq!(frames.stats().reserved, 16);
    assert_eq!(frames.stats().free, 112);
    Ok(())
}
```

`RegionMap` does not automatically configure a `BitmapFrames` pool or validate a
`MappingTable` against firmware records. Build each pool only from a verified
usable region and explicitly reserve every excluded frame before allocation.
Cross-resource transactions are a caller responsibility: if mapping a newly
allocated extent fails, release that allocation explicitly after accounting for
any other owners. Do not free memory still reachable through another mapping.

## Caller-owned bootstrap bytes

`BumpArena` is a temporary, initialized byte-slice arena, not `GlobalAlloc` or a
heap. Allocation honors the actual backing-pointer alignment of `Layout`, including
leading padding. A returned slice borrows the arena mutably: another allocation or
rollback is not allowed while that slice remains in use. Zero-size allocations
are rejected. The full backing bytes remain inspectable through `contents()`.

```rust
use core::alloc::Layout;
use spark_os::memory::{copy_bytes, move_bytes, zero_bytes, BumpArena, MemoryError};

fn main() -> Result<(), MemoryError> {
    let mut storage = [0xa5; 128];
    let mut arena = BumpArena::new(&mut storage);
    let checkpoint = arena.checkpoint();
    let layout = Layout::from_size_align(32, 16).unwrap();
    let block = arena.allocate(layout)?;
    assert_eq!(block.as_ptr() as usize % 16, 0);
    copy_bytes(block, 0, b"spark", 0, 5)?;
    move_bytes(block, 0, 2, 5)?; // Source offset, destination offset, byte length.
    assert_eq!(&block[2..7], b"spark");
    zero_bytes(block, 7, 25)?;
    let consumed = arena.used(); // The last use of block was above.
    arena.rollback(checkpoint)?;
    assert_eq!(arena.used(), 0);
    assert!(arena.contents()[..consumed].iter().all(|byte| *byte == 0));
    assert_eq!(arena.rollback(checkpoint), Err(MemoryError::InvalidCheckpoint));
    Ok(())
}
```

A checkpoint is scoped to its live backing slice, capacity, position, and rollback
epoch. Successful rollback zeros discarded bytes **including alignment padding**
and invalidates all previous checkpoints. Foreign, later-position, and stale-epoch
checkpoints fail without mutation. Do not retain a checkpoint after destroying
and recreating its arena: identity is not a globally unique lifetime token.
Zeroing is ordinary RAM initialization, not a volatile cryptographic secret wipe.

The standalone byte APIs use checked byte offsets and validate all ranges before
writing:

- `zero_bytes(destination, offset, len)` clears a subrange.
- `copy_bytes(destination, destination_offset, source, source_offset, len)` copies
  between safely nonaliasing slices.
- `move_bytes(bytes, source_offset, destination_offset, len)` supports overlap in
  either direction using snapshot-equivalent/memmove semantics.

An empty range at a slice's exclusive end is valid, including an empty slice.
Offsets beyond the end are invalid even when `len == 0`. Offset-plus-length
arithmetic overflow yields `Overflow`; otherwise invalid bounds yield
`OutOfRange`. Errors leave destination bytes unchanged. These functions must not
be used as volatile MMIO accessors.

## Public API and invariants

All public items are reexported through `spark_os::memory`:

- `align_down`, `align_up`, `is_aligned`: nonzero power-of-two alignments only;
  upward rounding fails rather than wrapping.
- `PhysAddr`, `VirtAddr`, `PAGE_SIZE`: distinct address types with checked
  addition, subtraction, alignment, and page offsets. `new` wraps an integer; it
  does not validate hardware physical width or virtual-address canonicality.
- `PageRange`: aligned half-open virtual intervals, byte-covering construction,
  page iteration, overlap and containment. Empty ranges are allowed; empty or
  adjacent intervals never overlap. Exclusive ends must fit in `usize`, so a
  full page whose exclusive end would wrap cannot be described.
- `FrameExtent`: a checked nonempty physical interval; it describes addresses and
  is not an ownership capability. `BitmapFrames` allocates lowest-first single,
  contiguous, or physically aligned extents from two separate mutable `u64`
  bitmaps. Each needs `ceil(frame_count / 64)` words; successful construction
  clears all supplied bitmap words. Unused tail bits never become frames.
- `FrameStats`: recomputed `total`, client `allocated`, permanent `reserved`,
  `free`, and `largest_free_run` counts, all measured in frames.
- `MemoryKind`, `CachePolicy`, `RegionAttributes`, `MemoryRegion`, `RegionMap<N>`:
  checked page-aligned usable/reserved/device records in a fixed-capacity,
  sorted, nonoverlapping physical-region map. Device regions require device
  cache policy; RAM must use normal cache policy. Maps may have zero capacity.
- `BumpArena`, `BumpCheckpoint`: borrowed byte allocations and zeroing rollback.
- `PagePermissions`, `PageDescriptor`, `TableDescriptor`: readable W^X permissions
  and pure AArch64 stage-1 descriptor encoding, described below.
- `Mapping`, `MappingTable<N>`, `AccessType`: sorted, fixed-capacity software
  translation and permission plans, described below.
- `zero_bytes`, `copy_bytes`, `move_bytes`, `self_test`, `MemoryError`: checked byte
  operations, the runtime smoke test, and comparable typed failure results.

Frame reservations are permanent and cannot be freed through the allocation
release APIs. Reservation, allocation, and extent release validate their complete
operation before mutation. Releasing any reserved or already-free frame fails
without partially releasing the extent. Stale frame addresses are **not** detected
after reuse; callers must enforce ownership and serialize shared allocator access.
The allocator never zeros physical frames or supplies references to their bytes.

### Mapping and access policy

`Mapping::new(range, physical, permissions, cache)` requires nonempty extents,
aligned physical frames, and representable physical and virtual ends. Device
mappings and device leaf descriptors are kernel-only and nonexecutable.
`PagePermissions::new(writable, executable, user)` always implies readability and
rejects writable-plus-executable permissions.

`MappingTable<N>` stores disjoint virtual extents in sorted order:

- `map(mapping)` rejects virtual overlap and exhausted capacity without mutation.
- Physical overlap is permitted only if aliases have identical cache policy and
  identical `user` access. Writable and executable aliases to any shared physical
  page cannot coexist, including partially overlapping extents.
- `lookup(address)` finds an extent, `translate(address)` preserves its byte
  offset without checking permissions, and `resolve(address, access, from_user)`
  checks read/write/execute and user access first. Missing bytes yield `NotMapped`;
  denied access yields `PermissionDenied`.
- Kernel reads/writes of user-accessible pages are modeled as allowed, subject to
  write permission. Execution is permitted only at the selected privilege level:
  user code is PXN and kernel code is UXN. PAN, privilege overrides, and actual
  exception-level or MMU state are not modeled.
- `unmap(range)` removes and returns exactly one stored extent, compacts the
  remaining entries, and recycles the slot. It does not release physical frames.
- `protect(range, permissions)` returns the previous mapping and replaces only
  permissions, after rechecking device and physical-alias policy. It works at full
  capacity and excludes the mapping being replaced from alias checks. Physical
  addresses, cache policy, ordering, and length remain unchanged.
- Both exact-range operations reject empty ranges with `InvalidSize`; partial,
  combined, and absent extents yield `NotMapped`. There is no implicit splitting
  or coalescing. Every failed update preserves the complete table.

Alias enforcement is local to one table. It cannot see mappings in other address
spaces or descriptors installed elsewhere. `Mapping` alone cannot enforce alias
policy. `MappingTable` does not own frames, install descriptors, update cache
state, invalidate TLBs, or synchronize CPUs. A live hardware W-to-X transition
requires the platform's own ordering, cache maintenance, and break-before-make
protocol; changing this offline plan does not perform that protocol.

### AArch64 descriptor contract

`PageDescriptor` encodes a valid/access-flag-set level-3 leaf for the 4 KiB granule
and physical addresses below 2^48. `TableDescriptor` encodes an aligned next-level
table address without hierarchical restrictions. Both reject misalignment and
unsupported physical width; their checked physical width is narrower than the
general address wrappers. Descriptor values are integers, not installed tables.

The platform must configure matching MAIR entries:

- Index 0: normal write-back (`CachePolicy::WriteBack`).
- Index 1: normal non-cacheable (`CachePolicy::NonCacheable`).
- Index 2: Device-nGnRnE (`CachePolicy::Device`).

Normal memory uses inner-shareable bits; devices use outer-shareable bits. User
leaf entries are non-global. Privileged/unprivileged execute-never and AP bits
follow the same permission model used by software resolution. The caller must
provide actual table memory, initialize it, configure translation controls, and
perform barriers, TLB invalidation, cache maintenance, and CPU synchronization.

## Verification

Run from the repository root with the pinned toolchain installed. The first two
commands build without Cargo or registry access. The library contains no heap or
external dependency requirement; the legacy board binary has separate build needs.

```sh
sh tools/host-test.sh
rustc --edition=2021 --crate-name spark_os --crate-type lib src/lib.rs \
  -D warnings -D missing-docs -o target/host/libspark_os.rlib
rustdoc --edition=2021 --crate-name spark_os --test src/lib.rs \
  --extern spark_os=target/host/libspark_os.rlib -D warnings -D missing-docs
rustc --edition=2021 --crate-name spark_os --crate-type lib src/lib.rs \
  --target aarch64-unknown-none-softfloat -D warnings -D missing-docs \
  -o target/host/libspark_os-aarch64.rlib
```

Host tests include boundary and lifecycle regressions plus deterministic reference
models: per-frame allocator states, page-enumerated mapping aliases and access,
byte/epoch arena state, and exhaustive small-range byte copies and moves. Three
fixed seeds drive each stateful model. Their checks were also exercised against
intentional mutations of allocated-frame occupancy, cross-alias W^X enforcement,
and rollback zeroing; each mutation failed its corresponding model test and was
reverted. Cross-compiling this library is not a kernel boot or hardware MMU test.
