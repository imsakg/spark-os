//! Deterministic reference models; no registry dependencies or timing assumptions.
use super::*;
use std::vec::Vec;

fn next(state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 16) as usize
}

#[test]
fn frame_operations_match_a_per_frame_reference_model() {
    const COUNT: usize = 70;
    for seed in [1u64, 0x1234_5678, 0xdead_beef] {
        let mut random = seed;
        let base = (1 + next(&mut random) % 7) * PAGE_SIZE;
        let mut used = [0; 2];
        let mut reserved = [0; 2];
        let mut frames =
            BitmapFrames::new(PhysAddr::new(base), COUNT, &mut used, &mut reserved).unwrap();
        // 0 = free, 1 = client allocation, 2 = permanent reservation.
        let mut model = [0u8; COUNT];
        frames
            .reserve(PhysAddr::new(base + 31 * PAGE_SIZE), 3)
            .unwrap();
        model[31..34].fill(2);
        let mut successes = [0usize, 0, 1];
        for step in 0..2048 {
            let operation = next(&mut random) % 5;
            let count = next(&mut random) % 7;
            if operation < 2 {
                let alignment = PAGE_SIZE << (next(&mut random) % 4);
                let expected = if count == 0 {
                    Err(MemoryError::InvalidSize)
                } else {
                    (0..=COUNT - count)
                        .find(|start| {
                            (base + start * PAGE_SIZE) % alignment == 0
                                && model[*start..*start + count]
                                    .iter()
                                    .all(|state| *state == 0)
                        })
                        .ok_or(MemoryError::OutOfMemory)
                };
                let actual = frames.allocate_aligned(count, alignment);
                assert_eq!(
                    actual.map(|extent| (extent.start().get() - base) / PAGE_SIZE),
                    expected,
                    "allocation seed={seed} step={step}"
                );
                if let Ok(start) = expected {
                    model[start..start + count].fill(1);
                    successes[0] += 1;
                }
            } else {
                let start = next(&mut random) % (COUNT + 2);
                let reserving = operation == 4 && step < 24;
                let expected = if count == 0 {
                    Err(MemoryError::InvalidSize)
                } else if start + count > COUNT {
                    Err(MemoryError::OutOfRange)
                } else if reserving {
                    if model[start..start + count].iter().any(|state| *state != 0) {
                        Err(MemoryError::AlreadyUsed)
                    } else {
                        Ok(())
                    }
                } else {
                    match model[start..start + count]
                        .iter()
                        .find(|state| **state != 1)
                    {
                        Some(2) => Err(MemoryError::Reserved),
                        Some(_) => Err(MemoryError::DoubleFree),
                        None => Ok(()),
                    }
                };
                let address = PhysAddr::new(base + start * PAGE_SIZE);
                let actual = if reserving {
                    frames.reserve(address, count)
                } else {
                    FrameExtent::new(address, count).and_then(|extent| frames.free_extent(extent))
                };
                assert_eq!(actual, expected, "release/reserve seed={seed} step={step}");
                if expected.is_ok() {
                    model[start..start + count].fill(if reserving { 2 } else { 0 });
                    successes[if reserving { 2 } else { 1 }] += 1;
                }
            }
            let expected = FrameStats {
                total: COUNT,
                allocated: model.iter().filter(|state| **state == 1).count(),
                reserved: model.iter().filter(|state| **state == 2).count(),
                free: model.iter().filter(|state| **state == 0).count(),
                largest_free_run: model
                    .split(|state| *state != 0)
                    .map(|run| run.len())
                    .max()
                    .unwrap_or(0),
            };
            assert_eq!(
                frames.stats(),
                expected,
                "accounting seed={seed} step={step}"
            );
            for (index, state) in model.iter().enumerate() {
                assert_eq!(
                    frames.is_allocated(PhysAddr::new(base + index * PAGE_SIZE)),
                    Ok(*state != 0)
                );
            }
        }
        assert!(
            successes.iter().all(|count| *count > 0),
            "seed={seed}: {successes:?}"
        );
    }
}

// The alias oracle enumerates physical page identities rather than using the
// implementation's interval-intersection predicate.
fn alias_oracle(
    candidate: Mapping,
    model: &[Mapping],
    skip: Option<usize>,
) -> Result<(), MemoryError> {
    for (index, entry) in model.iter().enumerate() {
        if skip == Some(index) {
            continue;
        }
        let shared = (0..candidate.range().page_count()).any(|left| {
            (0..entry.range().page_count()).any(|right| {
                candidate.physical_start().get() + left * PAGE_SIZE
                    == entry.physical_start().get() + right * PAGE_SIZE
            })
        });
        if !shared {
            continue;
        }
        if candidate.cache() != entry.cache() {
            return Err(MemoryError::InvalidAttributes);
        }
        let left = candidate.permissions();
        let right = entry.permissions();
        if left.user() != right.user()
            || (left.writable() && right.executable())
            || (right.writable() && left.executable())
        {
            return Err(MemoryError::InvalidPermissions);
        }
    }
    Ok(())
}

#[test]
fn mapping_operations_match_an_enumerated_page_reference_model() {
    let permissions = [
        PagePermissions::new(true, false, false).unwrap(),
        PagePermissions::new(false, true, false).unwrap(),
        PagePermissions::new(false, false, false).unwrap(),
        PagePermissions::new(false, false, true).unwrap(),
        PagePermissions::new(true, false, true).unwrap(),
        PagePermissions::new(false, true, true).unwrap(),
    ];
    let caches = [
        CachePolicy::WriteBack,
        CachePolicy::NonCacheable,
        CachePolicy::Device,
    ];
    for seed in [7u64, 0x1020_3040, 0xfeed_face] {
        let mut random = seed;
        let mut table = MappingTable::<6>::new();
        let mut model: Vec<Mapping> = Vec::new();
        let mut successes = [0usize; 3];
        for step in 0..2048 {
            let operation = next(&mut random) % 3;
            let requested = permissions[next(&mut random) % permissions.len()];
            let mut range = PageRange::new(
                VirtAddr::new((1 + next(&mut random) % 12) * PAGE_SIZE),
                next(&mut random) % 4,
            )
            .unwrap();
            if operation != 0 && !model.is_empty() && next(&mut random) % 3 != 0 {
                range = model[next(&mut random) % model.len()].range();
            }
            match operation {
                0 => {
                    let physical = PhysAddr::new((8 + next(&mut random) % 8) * PAGE_SIZE);
                    let cache = caches[next(&mut random) % caches.len()];
                    if let Ok(candidate) = Mapping::new(range, physical, requested, cache) {
                        let virtual_overlap = range.pages().any(|page| {
                            model
                                .iter()
                                .any(|entry| entry.range().pages().any(|other| page == other))
                        });
                        let expected = if virtual_overlap {
                            Err(MemoryError::AlreadyUsed)
                        } else {
                            alias_oracle(candidate, &model, None).and_then(|()| {
                                if model.len() == 6 {
                                    Err(MemoryError::Capacity)
                                } else {
                                    Ok(())
                                }
                            })
                        };
                        assert_eq!(
                            table.map(candidate),
                            expected,
                            "map seed={seed} step={step}"
                        );
                        if expected.is_ok() {
                            model.push(candidate);
                            model.sort_by_key(|entry| entry.range().start());
                            successes[0] += 1;
                        }
                    }
                }
                1 | 2 => {
                    let index = if range.is_empty() {
                        Err(MemoryError::InvalidSize)
                    } else {
                        model
                            .iter()
                            .position(|entry| entry.range() == range)
                            .ok_or(MemoryError::NotMapped)
                    };
                    if operation == 1 {
                        let expected = index.map(|index| model[index]);
                        assert_eq!(
                            table.unmap(range),
                            expected,
                            "unmap seed={seed} step={step}"
                        );
                        if let Ok(index) = index {
                            model.remove(index);
                            successes[1] += 1;
                        }
                    } else {
                        let expected = index.and_then(|index| {
                            let old = model[index];
                            if old.cache() == CachePolicy::Device
                                && (requested.user() || requested.executable())
                            {
                                return Err(MemoryError::InvalidPermissions);
                            }
                            let candidate =
                                Mapping::new(range, old.physical_start(), requested, old.cache())
                                    .unwrap();
                            alias_oracle(candidate, &model, Some(index)).map(|()| old)
                        });
                        assert_eq!(
                            table.protect(range, requested),
                            expected,
                            "protect seed={seed} step={step}"
                        );
                        if let Ok(old) = expected {
                            model[index.unwrap()] =
                                Mapping::new(range, old.physical_start(), requested, old.cache())
                                    .unwrap();
                            successes[2] += 1;
                        }
                    }
                }
                _ => unreachable!(),
            }
            assert_eq!(table.len(), model.len());
            assert_eq!(
                table.mappings().collect::<Vec<_>>(),
                model,
                "plan seed={seed} step={step}"
            );
            // Sample a byte, including holes, interiors, and exclusive ends.
            let address =
                VirtAddr::new((next(&mut random) % 17) * PAGE_SIZE + next(&mut random) % PAGE_SIZE);
            let mapping = model.iter().find(|entry| {
                let start = entry.range().start().get();
                address.get() >= start
                    && address.get() - start < entry.range().page_count() * PAGE_SIZE
            });
            let translated = mapping
                .map(|entry| {
                    PhysAddr::new(
                        entry.physical_start().get() + address.get() - entry.range().start().get(),
                    )
                })
                .ok_or(MemoryError::NotMapped);
            assert_eq!(table.translate(address), translated);
            for from_user in [false, true] {
                for access in [AccessType::Read, AccessType::Write, AccessType::Execute] {
                    let expected = match mapping {
                        None => Err(MemoryError::NotMapped),
                        Some(entry) => {
                            let p = entry.permissions();
                            let allowed = (!from_user || p.user())
                                && match access {
                                    AccessType::Read => true,
                                    AccessType::Write => p.writable(),
                                    AccessType::Execute => p.executable() && p.user() == from_user,
                                };
                            if allowed {
                                translated
                            } else {
                                Err(MemoryError::PermissionDenied)
                            }
                        }
                    };
                    assert_eq!(table.resolve(address, access, from_user), expected);
                }
            }
        }
        assert!(
            successes.iter().all(|count| *count > 0),
            "seed={seed}: {successes:?}"
        );
    }
}

#[test]
fn exhaustive_byte_operations_match_snapshot_reference_copies() {
    let original = [0u8, 1, 2, 3, 4, 5, 6, 7];
    for source in 0..=10 {
        for destination in 0..=10 {
            for len in 0..=10 {
                let valid = source + len <= original.len() && destination + len <= original.len();
                let expected_result = if valid {
                    Ok(())
                } else {
                    Err(MemoryError::OutOfRange)
                };
                let mut expected = original;
                if valid {
                    for index in 0..len {
                        expected[destination + index] = original[source + index];
                    }
                }
                let mut moved = original;
                assert_eq!(
                    move_bytes(&mut moved, source, destination, len),
                    expected_result
                );
                assert_eq!(moved, expected);
                let mut copied = original;
                assert_eq!(
                    copy_bytes(&mut copied, destination, &original, source, len),
                    expected_result
                );
                assert_eq!(copied, expected);
            }
        }
        for len in 0..=10 {
            let valid = source + len <= original.len();
            let mut expected = original;
            if valid {
                for byte in &mut expected[source..source + len] {
                    *byte = 0;
                }
            }
            let mut zeroed = original;
            assert_eq!(
                zero_bytes(&mut zeroed, source, len),
                if valid {
                    Ok(())
                } else {
                    Err(MemoryError::OutOfRange)
                }
            );
            assert_eq!(zeroed, expected);
        }
    }
}

#[test]
fn arena_allocations_and_checkpoint_epochs_match_a_byte_reference_model() {
    use core::alloc::Layout;
    for seed in [9u64, 0xabcdef, 0x7654_3210] {
        let mut random = seed;
        let mut backing = [0xa5; 128];
        let base = backing.as_ptr() as usize;
        let mut foreign_backing = [0u8; 128];
        let foreign_arena = BumpArena::new(&mut foreign_backing);
        let foreign = foreign_arena.checkpoint();
        let mut arena = BumpArena::new(&mut backing);
        let mut bytes = [0xa5; 128];
        let mut cursor = 0;
        let mut epoch = 0;
        let mut checkpoints = std::vec![(arena.checkpoint(), 0, 0)];
        let mut successes = [0usize; 2];
        for step in 0..1024 {
            match next(&mut random) % 4 {
                0 => checkpoints.push((arena.checkpoint(), cursor, epoch)),
                1 => {
                    let size = next(&mut random) % 40;
                    let alignment = 1 << (next(&mut random) % 6);
                    let start = ((base + cursor + alignment - 1) / alignment) * alignment - base;
                    let end = start + size;
                    let expected = if size == 0 {
                        Err(MemoryError::InvalidSize)
                    } else if end > bytes.len() {
                        Err(MemoryError::OutOfMemory)
                    } else {
                        Ok(())
                    };
                    let result = arena.allocate(Layout::from_size_align(size, alignment).unwrap());
                    assert_eq!(
                        result.as_ref().map(|_| ()).map_err(|error| *error),
                        expected,
                        "allocate seed={seed} step={step}"
                    );
                    if let Ok(block) = result {
                        assert_eq!(block.as_ptr() as usize, base + start);
                        assert_eq!(block.len(), size);
                        let value = next(&mut random) as u8;
                        block.fill(value);
                        bytes[start..end].fill(value);
                        cursor = end;
                        successes[0] += 1;
                    }
                }
                2 => {
                    let (checkpoint, saved, saved_epoch) =
                        checkpoints[next(&mut random) % checkpoints.len()];
                    let expected = if saved_epoch != epoch || saved > cursor {
                        Err(MemoryError::InvalidCheckpoint)
                    } else {
                        Ok(())
                    };
                    assert_eq!(
                        arena.rollback(checkpoint),
                        expected,
                        "rollback seed={seed} step={step}"
                    );
                    if expected.is_ok() {
                        bytes[saved..cursor].fill(0);
                        cursor = saved;
                        epoch += 1;
                        successes[1] += 1;
                    }
                }
                _ => assert_eq!(arena.rollback(foreign), Err(MemoryError::InvalidCheckpoint)),
            }
            assert_eq!(arena.used(), cursor, "cursor seed={seed} step={step}");
            assert_eq!(arena.remaining(), bytes.len() - cursor);
            assert_eq!(arena.contents(), bytes, "bytes seed={seed} step={step}");
        }
        assert!(
            successes.iter().all(|count| *count > 0),
            "seed={seed}: {successes:?}"
        );
    }
}
