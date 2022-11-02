use super::*;

#[test]
fn alignment_is_checked_at_boundaries() {
    assert_eq!(align_down(0x1234, 4096), Ok(0x1000));
    assert_eq!(align_up(0x1234, 4096), Ok(0x2000));
    assert_eq!(align_up(4096, 4096), Ok(4096));
    assert_eq!(align_up(usize::MAX, 1), Ok(usize::MAX));
    assert_eq!(align_up(usize::MAX, 2), Err(MemoryError::Overflow));
    for alignment in [0, 3, 6] {
        assert_eq!(align_up(7, alignment), Err(MemoryError::InvalidAlignment));
        assert_eq!(align_down(7, alignment), Err(MemoryError::InvalidAlignment));
    }
    assert_eq!(is_aligned(0x1000, 4096), Ok(true));
    assert_eq!(is_aligned(0x1001, 4096), Ok(false));
}

#[test]
fn address_types_check_arithmetic_and_offsets() {
    let p = PhysAddr::new(0x1234);
    let v = VirtAddr::new(0x8000);
    assert_eq!(p.get(), 0x1234);
    assert_eq!(v.checked_add(4096).unwrap().get(), 0x9000);
    assert_eq!(p.align_down(4096).unwrap().get(), 0x1000);
    assert_eq!(p.align_up(4096).unwrap().get(), 0x2000);
    assert_eq!(v.page_offset(), 0);
    assert_eq!(p.page_offset(), 0x234);
    assert_eq!(
        PhysAddr::new(usize::MAX).checked_add(1),
        Err(MemoryError::Overflow)
    );
    assert_eq!(VirtAddr::new(0).checked_sub(1), Err(MemoryError::Overflow));
    assert_eq!(v.align_up(0), Err(MemoryError::InvalidAlignment));
    assert_eq!(p.checked_sub(4).unwrap().get(), 0x1230);
    assert_eq!(v.align_down(4096).unwrap(), v);
}

#[test]
fn page_ranges_are_checked_half_open_intervals() {
    let range = PageRange::new(VirtAddr::new(0x1000), 3).unwrap();
    assert_eq!(range.start(), VirtAddr::new(0x1000));
    assert_eq!(range.end(), VirtAddr::new(0x4000));
    assert_eq!(range.page_count(), 3);
    assert_eq!(range.byte_len(), 3 * PAGE_SIZE);
    assert!(range.contains(VirtAddr::new(0x3fff)));
    assert!(!range.contains(range.end()));
    assert_eq!(
        range
            .pages()
            .map(VirtAddr::get)
            .collect::<std::vec::Vec<_>>(),
        [0x1000, 0x2000, 0x3000]
    );
    let cover = PageRange::covering(VirtAddr::new(0x1fff), 2).unwrap();
    assert_eq!(cover.page_count(), 2);
    assert!(range.overlaps(cover));
    assert!(!range.overlaps(PageRange::new(range.end(), 1).unwrap()));
    let empty = PageRange::covering(VirtAddr::new(3), 0).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.pages().len(), 0);
    assert!(!empty.overlaps(range));
    assert_eq!(
        PageRange::new(VirtAddr::new(1), 1),
        Err(MemoryError::Misaligned)
    );
    assert_eq!(
        PageRange::new(VirtAddr::new(0), usize::MAX),
        Err(MemoryError::Overflow)
    );
    assert_eq!(
        PageRange::covering(VirtAddr::new(usize::MAX), 1),
        Err(MemoryError::Overflow)
    );
}

#[test]
fn bitmap_frames_exhaust_without_allocating_tail_bits() {
    let mut used = [u64::MAX; 2];
    let mut reserved = [u64::MAX; 2];
    let mut frames =
        BitmapFrames::new(PhysAddr::new(0x1000), 65, &mut used, &mut reserved).unwrap();
    assert_eq!(frames.base(), PhysAddr::new(0x1000));
    assert_eq!(frames.frame_count(), 65);
    for i in 0..65 {
        assert_eq!(frames.allocate(), Ok(PhysAddr::new(0x1000 + i * PAGE_SIZE)));
    }
    assert_eq!(frames.allocate(), Err(MemoryError::OutOfMemory));
    assert_eq!(frames.is_allocated(PhysAddr::new(0x1000)), Ok(true));
}

#[test]
fn bitmap_constructor_rejects_invalid_backing_without_mutation() {
    let mut a = [123];
    let mut b = [456];
    assert_eq!(
        BitmapFrames::new(PhysAddr::new(1), 1, &mut a, &mut b).err(),
        Some(MemoryError::Misaligned)
    );
    assert_eq!(
        BitmapFrames::new(PhysAddr::new(0), 65, &mut a, &mut b).err(),
        Some(MemoryError::Capacity)
    );
    assert_eq!(
        BitmapFrames::new(PhysAddr::new(0), 0, &mut a, &mut b).err(),
        Some(MemoryError::InvalidSize)
    );
    assert_eq!(
        BitmapFrames::new(PhysAddr::new(0), usize::MAX, &mut a, &mut b).err(),
        Some(MemoryError::Overflow)
    );
    assert_eq!(a, [123]);
    assert_eq!(b, [456]);
}

#[test]
fn freeing_frames_reuses_lowest_hole_and_rejects_invalid_frees() {
    let mut a = [0; 2];
    let mut b = [0; 2];
    let mut frames = BitmapFrames::new(PhysAddr::new(PAGE_SIZE), 66, &mut a, &mut b).unwrap();
    let first = frames.allocate().unwrap();
    let second = frames.allocate().unwrap();
    assert_eq!(frames.free(first), Ok(()));
    assert_eq!(frames.free(first), Err(MemoryError::DoubleFree));
    assert_eq!(frames.free(PhysAddr::new(0)), Err(MemoryError::OutOfRange));
    assert_eq!(
        frames.free(PhysAddr::new(67 * PAGE_SIZE)),
        Err(MemoryError::OutOfRange)
    );
    assert_eq!(
        frames.free(PhysAddr::new(PAGE_SIZE + 1)),
        Err(MemoryError::Misaligned)
    );
    assert_eq!(frames.is_allocated(second), Ok(true));
    assert_eq!(frames.allocate(), Ok(first));
    for _ in 2..66 {
        frames.allocate().unwrap();
    }
    let tail = PhysAddr::new(66 * PAGE_SIZE);
    frames.free(tail).unwrap();
    assert_eq!(frames.allocate(), Ok(tail));
    assert_eq!(frames.allocate(), Err(MemoryError::OutOfMemory));
}

#[test]
fn frame_reservations_are_atomic_and_cannot_be_freed() {
    let mut a = [0; 2];
    let mut b = [0; 2];
    let mut frames = BitmapFrames::new(PhysAddr::new(0), 70, &mut a, &mut b).unwrap();
    frames.reserve(PhysAddr::new(63 * PAGE_SIZE), 3).unwrap();
    assert_eq!(
        frames.free(PhysAddr::new(64 * PAGE_SIZE)),
        Err(MemoryError::Reserved)
    );
    assert_eq!(
        frames.reserve(PhysAddr::new(62 * PAGE_SIZE), 2),
        Err(MemoryError::AlreadyUsed)
    );
    assert_eq!(
        frames.is_allocated(PhysAddr::new(62 * PAGE_SIZE)),
        Ok(false)
    );
    assert_eq!(
        frames.reserve(PhysAddr::new(69 * PAGE_SIZE), 2),
        Err(MemoryError::OutOfRange)
    );
    assert_eq!(
        frames.is_allocated(PhysAddr::new(69 * PAGE_SIZE)),
        Ok(false)
    );
    assert_eq!(
        frames.reserve(PhysAddr::new(0), 0),
        Err(MemoryError::InvalidSize)
    );
    let first = frames.allocate().unwrap();
    assert_eq!(frames.reserve(first, 1), Err(MemoryError::AlreadyUsed));
    for i in 1..63 {
        assert_eq!(frames.allocate(), Ok(PhysAddr::new(i * PAGE_SIZE)));
    }
    assert_eq!(frames.allocate(), Ok(PhysAddr::new(66 * PAGE_SIZE)));
}

#[test]
fn frame_accounting_distinguishes_reservations_and_fragmentation() {
    let mut a = [0];
    let mut b = [0];
    let mut frames = BitmapFrames::new(PhysAddr::new(0), 8, &mut a, &mut b).unwrap();
    assert_eq!(
        frames.stats(),
        FrameStats {
            total: 8,
            allocated: 0,
            reserved: 0,
            free: 8,
            largest_free_run: 8
        }
    );
    frames.reserve(PhysAddr::new(2 * PAGE_SIZE), 2).unwrap();
    frames.allocate().unwrap();
    let second = frames.allocate().unwrap();
    assert_eq!(
        frames.stats(),
        FrameStats {
            total: 8,
            allocated: 2,
            reserved: 2,
            free: 4,
            largest_free_run: 4
        }
    );
    frames.free(second).unwrap();
    frames.reserve(PhysAddr::new(5 * PAGE_SIZE), 1).unwrap();
    assert_eq!(
        frames.stats(),
        FrameStats {
            total: 8,
            allocated: 1,
            reserved: 3,
            free: 4,
            largest_free_run: 2
        }
    );
    let before = frames.stats();
    assert_eq!(
        frames.reserve(PhysAddr::new(0), 8),
        Err(MemoryError::AlreadyUsed)
    );
    assert_eq!(frames.stats(), before);
}

#[test]
fn contiguous_frames_cross_bitmap_words_and_fail_without_partial_claims() {
    let mut a = [0; 2];
    let mut b = [0; 2];
    let mut frames = BitmapFrames::new(PhysAddr::new(PAGE_SIZE), 70, &mut a, &mut b).unwrap();
    frames.reserve(PhysAddr::new(PAGE_SIZE), 62).unwrap();
    let extent = frames.allocate_contiguous(5).unwrap();
    assert_eq!(extent.start(), PhysAddr::new(63 * PAGE_SIZE));
    assert_eq!(extent.end(), PhysAddr::new(68 * PAGE_SIZE));
    assert_eq!(extent.frame_count(), 5);
    assert_eq!(extent.byte_len(), 5 * PAGE_SIZE);
    let before = frames.stats();
    assert_eq!(frames.allocate_contiguous(4), Err(MemoryError::OutOfMemory));
    assert_eq!(frames.allocate_contiguous(0), Err(MemoryError::InvalidSize));
    assert_eq!(
        frames.allocate_contiguous(usize::MAX),
        Err(MemoryError::Overflow)
    );
    assert_eq!(frames.stats(), before);
    assert_eq!(frames.allocate_contiguous(3).unwrap().start(), extent.end());
    assert_eq!(frames.stats().free, 0);
    assert_eq!(
        FrameExtent::new(PhysAddr::new(1), 1),
        Err(MemoryError::Misaligned)
    );
}

#[test]
fn frame_extents_align_physical_addresses_not_pool_indices() {
    let mut a = [0];
    let mut b = [0];
    let mut frames = BitmapFrames::new(PhysAddr::new(PAGE_SIZE), 15, &mut a, &mut b).unwrap();
    let extent = frames.allocate_aligned(3, 4 * PAGE_SIZE).unwrap();
    assert_eq!(extent.start(), PhysAddr::new(4 * PAGE_SIZE));
    assert_eq!(frames.allocate(), Ok(PhysAddr::new(PAGE_SIZE)));
    assert_eq!(
        frames.allocate_aligned(2, 8 * PAGE_SIZE).unwrap().start(),
        PhysAddr::new(8 * PAGE_SIZE)
    );
    let before = frames.stats();
    for alignment in [0, 3 * PAGE_SIZE, PAGE_SIZE / 2] {
        assert_eq!(
            frames.allocate_aligned(1, alignment),
            Err(MemoryError::InvalidAlignment)
        );
    }
    assert_eq!(
        frames.allocate_aligned(1, 32 * PAGE_SIZE),
        Err(MemoryError::OutOfMemory)
    );
    assert_eq!(frames.stats(), before);
}

#[test]
fn extent_release_validates_every_frame_before_mutation() {
    let mut a = [0];
    let mut b = [0];
    let mut frames = BitmapFrames::new(PhysAddr::new(0), 8, &mut a, &mut b).unwrap();
    let extent = frames.allocate_contiguous(3).unwrap();
    frames.reserve(PhysAddr::new(3 * PAGE_SIZE), 1).unwrap();
    let before = frames.stats();
    assert_eq!(
        frames.free_extent(FrameExtent::new(PhysAddr::new(0), 4).unwrap()),
        Err(MemoryError::Reserved)
    );
    assert_eq!(frames.stats(), before);
    frames.free(PhysAddr::new(PAGE_SIZE)).unwrap();
    let before = frames.stats();
    assert_eq!(frames.free_extent(extent), Err(MemoryError::DoubleFree));
    assert_eq!(frames.stats(), before);
    assert_eq!(frames.allocate(), Ok(PhysAddr::new(PAGE_SIZE)));
    frames.free_extent(extent).unwrap();
    assert_eq!(frames.stats().allocated, 0);
    assert_eq!(
        frames.free_extent(FrameExtent::new(PhysAddr::new(7 * PAGE_SIZE), 2).unwrap()),
        Err(MemoryError::OutOfRange)
    );
    assert_eq!(frames.stats().reserved, 1);
}

#[test]
fn physical_regions_validate_size_kind_and_cache_attributes() {
    let ram = RegionAttributes::new(MemoryKind::Usable, CachePolicy::WriteBack).unwrap();
    assert!(ram.is_allocatable());
    assert_eq!(ram.kind(), MemoryKind::Usable);
    assert_eq!(ram.cache(), CachePolicy::WriteBack);
    assert_eq!(
        RegionAttributes::new(MemoryKind::Device, CachePolicy::WriteBack),
        Err(MemoryError::InvalidAttributes)
    );
    assert_eq!(
        RegionAttributes::new(MemoryKind::Usable, CachePolicy::Device),
        Err(MemoryError::InvalidAttributes)
    );
    let device = RegionAttributes::new(MemoryKind::Device, CachePolicy::Device).unwrap();
    assert!(!device.is_allocatable());
    let region = MemoryRegion::new(PhysAddr::new(PAGE_SIZE), 2 * PAGE_SIZE, ram).unwrap();
    assert_eq!(region.start(), PhysAddr::new(PAGE_SIZE));
    assert_eq!(region.end(), PhysAddr::new(3 * PAGE_SIZE));
    assert_eq!(region.byte_len(), 2 * PAGE_SIZE);
    assert_eq!(region.attributes(), ram);
    assert!(region.contains(PhysAddr::new(3 * PAGE_SIZE - 1)));
    assert!(!region.contains(region.end()));
    assert_eq!(
        MemoryRegion::new(PhysAddr::new(0), 0, ram),
        Err(MemoryError::InvalidSize)
    );
    assert_eq!(
        MemoryRegion::new(PhysAddr::new(0), 1, ram),
        Err(MemoryError::Misaligned)
    );
    assert_eq!(
        MemoryRegion::new(PhysAddr::new(usize::MAX & !(PAGE_SIZE - 1)), PAGE_SIZE, ram),
        Err(MemoryError::Overflow)
    );
}

#[test]
fn region_maps_sort_nonoverlapping_regions_and_preserve_failures() {
    let ram = RegionAttributes::new(MemoryKind::Usable, CachePolicy::WriteBack).unwrap();
    let reserved = RegionAttributes::new(MemoryKind::Reserved, CachePolicy::NonCacheable).unwrap();
    let low = MemoryRegion::new(PhysAddr::new(0), PAGE_SIZE, ram).unwrap();
    let high = MemoryRegion::new(PhysAddr::new(3 * PAGE_SIZE), PAGE_SIZE, ram).unwrap();
    let middle = MemoryRegion::new(PhysAddr::new(PAGE_SIZE), 2 * PAGE_SIZE, reserved).unwrap();
    let mut map = RegionMap::<3>::new();
    assert!(map.is_empty());
    map.insert(high).unwrap();
    map.insert(low).unwrap();
    map.insert(middle).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(
        map.regions().collect::<std::vec::Vec<_>>(),
        [low, middle, high]
    );
    assert_eq!(map.lookup(PhysAddr::new(PAGE_SIZE)), Some(middle));
    assert_eq!(map.lookup(PhysAddr::new(4 * PAGE_SIZE)), None);
    assert_eq!(map.usable_bytes(), 2 * PAGE_SIZE);
    assert_eq!(map.insert(low), Err(MemoryError::AlreadyUsed));
    assert_eq!(
        map.insert(MemoryRegion::new(PhysAddr::new(5 * PAGE_SIZE), PAGE_SIZE, ram).unwrap()),
        Err(MemoryError::Capacity)
    );
    assert_eq!(
        map.regions().collect::<std::vec::Vec<_>>(),
        [low, middle, high]
    );
    assert_eq!(
        RegionMap::<0>::new().insert(low),
        Err(MemoryError::Capacity)
    );
}

#[test]
fn bump_allocations_align_real_slice_addresses_and_preserve_failure_state() {
    use core::alloc::Layout;
    let mut backing = [0xa5; 128];
    let mut arena = BumpArena::new(&mut backing[1..]);
    assert_eq!(arena.capacity(), 127);
    assert_eq!(arena.used(), 0);
    let block = arena
        .allocate(Layout::from_size_align(13, 32).unwrap())
        .unwrap();
    assert_eq!(block.as_ptr() as usize % 32, 0);
    assert_eq!(block, &[0xa5; 13]);
    block.fill(7);
    let before = arena.used();
    assert_eq!(arena.remaining(), arena.capacity() - before);
    assert_eq!(
        arena
            .allocate(Layout::from_size_align(200, 1).unwrap())
            .err(),
        Some(MemoryError::OutOfMemory)
    );
    assert_eq!(
        arena.allocate(Layout::from_size_align(0, 1).unwrap()).err(),
        Some(MemoryError::InvalidSize)
    );
    assert_eq!(arena.used(), before);
    let remaining = arena.remaining();
    arena
        .allocate(Layout::from_size_align(remaining, 1).unwrap())
        .unwrap();
    assert_eq!(arena.remaining(), 0);
    assert_eq!(
        arena.allocate(Layout::from_size_align(1, 1).unwrap()).err(),
        Some(MemoryError::OutOfMemory)
    );
}

#[test]
fn bump_rollback_scrubs_padding_and_rejects_foreign_or_stale_checkpoints() {
    use core::alloc::Layout;
    let mut a = [0xa5; 128];
    let mut b = [0x5a; 128];
    let mut arena = BumpArena::new(&mut a);
    let foreign = BumpArena::new(&mut b).checkpoint();
    arena
        .allocate(Layout::from_size_align(4, 1).unwrap())
        .unwrap()
        .fill(9);
    let checkpoint = arena.checkpoint();
    arena
        .allocate(Layout::from_size_align(8, 64).unwrap())
        .unwrap()
        .fill(7);
    let end = arena.used();
    let before = arena.contents().to_vec();
    assert_eq!(arena.rollback(foreign), Err(MemoryError::InvalidCheckpoint));
    assert_eq!(arena.contents(), before);
    arena.rollback(checkpoint).unwrap();
    assert_eq!(arena.used(), 4);
    assert_eq!(&arena.contents()[..4], &[9; 4]);
    assert!(arena.contents()[4..end].iter().all(|byte| *byte == 0));
    assert!(arena.contents()[end..].iter().all(|byte| *byte == 0xa5));
    assert_eq!(
        arena.rollback(checkpoint),
        Err(MemoryError::InvalidCheckpoint)
    );
    let next = arena.checkpoint();
    arena
        .allocate(Layout::from_size_align(3, 1).unwrap())
        .unwrap();
    arena.rollback(next).unwrap();
    assert_eq!(arena.used(), 4);
}

#[test]
fn aarch64_descriptors_encode_addresses_and_permission_bits_only() {
    let data = PagePermissions::new(true, false, false).unwrap();
    let descriptor =
        PageDescriptor::new(PhysAddr::new(0x1234_5000), data, CachePolicy::WriteBack).unwrap();
    assert_eq!(descriptor.output_address(), PhysAddr::new(0x1234_5000));
    assert_eq!(descriptor.bits() & 0xfff, 3 | (3 << 8) | (1 << 10));
    assert_eq!(
        descriptor.bits() & ((1 << 53) | (1 << 54)),
        (1 << 53) | (1 << 54)
    );
    assert!(data.writable());
    assert!(!data.executable());
    assert!(!data.user());
    let user_code = PagePermissions::new(false, true, true).unwrap();
    let user = PageDescriptor::new(PhysAddr::new(0), user_code, CachePolicy::NonCacheable).unwrap();
    assert_eq!(
        user.bits() & 0xfff,
        3 | (1 << 2) | (3 << 6) | (3 << 8) | (1 << 10) | (1 << 11)
    );
    assert_eq!(user.bits() & ((1 << 53) | (1 << 54)), 1 << 53);
    assert_eq!(
        PagePermissions::new(true, true, false),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(
        PageDescriptor::new(PhysAddr::new(1), data, CachePolicy::WriteBack),
        Err(MemoryError::Misaligned)
    );
    assert_eq!(
        PageDescriptor::new(PhysAddr::new(1usize << 48), data, CachePolicy::WriteBack),
        Err(MemoryError::AddressWidth)
    );
    assert_eq!(
        PageDescriptor::new(PhysAddr::new(0), user_code, CachePolicy::Device),
        Err(MemoryError::InvalidPermissions)
    );
    let table = TableDescriptor::new(PhysAddr::new(0x4000)).unwrap();
    assert_eq!(table.bits(), 0x4003);
    assert_eq!(
        TableDescriptor::new(PhysAddr::new(0x4001)),
        Err(MemoryError::Misaligned)
    );
}

#[test]
fn mapping_tables_keep_checked_disjoint_virtual_extents() {
    let data = PagePermissions::new(true, false, false).unwrap();
    let first = Mapping::new(
        PageRange::new(VirtAddr::new(0x4000), 2).unwrap(),
        PhysAddr::new(0x1000),
        data,
        CachePolicy::WriteBack,
    )
    .unwrap();
    let second = Mapping::new(
        PageRange::new(VirtAddr::new(0x6000), 1).unwrap(),
        PhysAddr::new(0xa000),
        data,
        CachePolicy::WriteBack,
    )
    .unwrap();
    let mut table = MappingTable::<2>::new();
    assert!(table.is_empty());
    table.map(second).unwrap();
    table.map(first).unwrap();
    assert_eq!(table.len(), 2);
    assert_eq!(
        table.mappings().collect::<std::vec::Vec<_>>(),
        [first, second]
    );
    assert_eq!(first.physical_start(), PhysAddr::new(0x1000));
    assert_eq!(first.range().page_count(), 2);
    assert_eq!(first.permissions(), data);
    assert_eq!(first.cache(), CachePolicy::WriteBack);
    assert_eq!(table.map(first), Err(MemoryError::AlreadyUsed));
    let third = Mapping::new(
        PageRange::new(VirtAddr::new(0x8000), 1).unwrap(),
        PhysAddr::new(0xb000),
        data,
        CachePolicy::WriteBack,
    )
    .unwrap();
    assert_eq!(table.map(third), Err(MemoryError::Capacity));
    assert_eq!(
        MappingTable::<0>::new().map(first),
        Err(MemoryError::Capacity)
    );
    assert_eq!(
        Mapping::new(
            PageRange::new(VirtAddr::new(0), 0).unwrap(),
            PhysAddr::new(0),
            data,
            CachePolicy::WriteBack
        ),
        Err(MemoryError::InvalidSize)
    );
    assert_eq!(
        Mapping::new(
            first.range(),
            PhysAddr::new(usize::MAX & !(PAGE_SIZE - 1)),
            data,
            CachePolicy::WriteBack
        ),
        Err(MemoryError::Overflow)
    );
}

#[test]
fn translation_preserves_byte_offsets_and_enforces_access() {
    let mut table = MappingTable::<2>::new();
    let user_data = Mapping::new(
        PageRange::new(VirtAddr::new(0x4000), 2).unwrap(),
        PhysAddr::new(0x9000),
        PagePermissions::new(true, false, true).unwrap(),
        CachePolicy::WriteBack,
    )
    .unwrap();
    let code = Mapping::new(
        PageRange::new(VirtAddr::new(0x8000), 1).unwrap(),
        PhysAddr::new(0x1000),
        PagePermissions::new(false, true, false).unwrap(),
        CachePolicy::WriteBack,
    )
    .unwrap();
    table.map(user_data).unwrap();
    table.map(code).unwrap();
    assert_eq!(table.lookup(VirtAddr::new(0x4fff)), Some(user_data));
    assert_eq!(
        table.translate(VirtAddr::new(0x5123)),
        Ok(PhysAddr::new(0xa123))
    );
    assert_eq!(
        table.translate(VirtAddr::new(0x5fff)),
        Ok(PhysAddr::new(0xafff))
    );
    assert_eq!(
        table.translate(VirtAddr::new(0x6000)),
        Err(MemoryError::NotMapped)
    );
    assert_eq!(
        user_data.translate(VirtAddr::new(0x3fff)),
        Err(MemoryError::NotMapped)
    );
    assert_eq!(
        table.resolve(VirtAddr::new(0x4000), AccessType::Write, true),
        Ok(PhysAddr::new(0x9000))
    );
    assert_eq!(
        table.resolve(VirtAddr::new(0x4000), AccessType::Execute, true),
        Err(MemoryError::PermissionDenied)
    );
    assert_eq!(
        table.resolve(VirtAddr::new(0x8000), AccessType::Read, true),
        Err(MemoryError::PermissionDenied)
    );
    assert_eq!(
        table.resolve(VirtAddr::new(0x8000), AccessType::Write, false),
        Err(MemoryError::PermissionDenied)
    );
    assert_eq!(
        table.resolve(VirtAddr::new(0x8123), AccessType::Execute, false),
        Ok(PhysAddr::new(0x1123))
    );
}

#[test]
fn physical_aliases_cannot_bypass_wx_cache_or_privilege_policy() {
    let range = |start, count| PageRange::new(VirtAddr::new(start), count).unwrap();
    let data = PagePermissions::new(true, false, false).unwrap();
    let code = PagePermissions::new(false, true, false).unwrap();
    let ro = PagePermissions::new(false, false, false).unwrap();
    let user = PagePermissions::new(false, false, true).unwrap();
    let mut table = MappingTable::<8>::new();
    table
        .map(
            Mapping::new(
                range(0x1000, 2),
                PhysAddr::new(0x8000),
                data,
                CachePolicy::WriteBack,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        table.map(
            Mapping::new(
                range(0x4000, 2),
                PhysAddr::new(0x9000),
                code,
                CachePolicy::WriteBack
            )
            .unwrap()
        ),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(
        table.map(
            Mapping::new(
                range(0x4000, 1),
                PhysAddr::new(0x8000),
                ro,
                CachePolicy::NonCacheable
            )
            .unwrap()
        ),
        Err(MemoryError::InvalidAttributes)
    );
    assert_eq!(
        table.map(
            Mapping::new(
                range(0x4000, 1),
                PhysAddr::new(0x8000),
                user,
                CachePolicy::WriteBack
            )
            .unwrap()
        ),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(table.len(), 1);
    table
        .map(
            Mapping::new(
                range(0x4000, 1),
                PhysAddr::new(0x8000),
                ro,
                CachePolicy::WriteBack,
            )
            .unwrap(),
        )
        .unwrap();
    table
        .map(
            Mapping::new(
                range(0x6000, 1),
                PhysAddr::new(0xa000),
                code,
                CachePolicy::WriteBack,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        table.map(
            Mapping::new(
                range(0x7000, 1),
                PhysAddr::new(0xa000),
                data,
                CachePolicy::WriteBack
            )
            .unwrap()
        ),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(
        Mapping::new(
            range(0x8000, 1),
            PhysAddr::new(0),
            user,
            CachePolicy::Device
        ),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(
        PageDescriptor::new(PhysAddr::new(0), user, CachePolicy::Device),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(table.len(), 3);
}

#[test]
fn unmap_requires_exact_extents_and_reuses_compacted_slots() {
    let make = |start| {
        Mapping::new(
            PageRange::new(VirtAddr::new(start), 2).unwrap(),
            PhysAddr::new(start + 0x10000),
            PagePermissions::new(true, false, false).unwrap(),
            CachePolicy::WriteBack,
        )
        .unwrap()
    };
    let low = make(0x1000);
    let middle = make(0x4000);
    let high = make(0x8000);
    let mut table = MappingTable::<3>::new();
    for mapping in [high, low, middle] {
        table.map(mapping).unwrap();
    }
    let before = table.mappings().collect::<std::vec::Vec<_>>();
    assert_eq!(
        table.unmap(PageRange::new(VirtAddr::new(0x4000), 0).unwrap()),
        Err(MemoryError::InvalidSize)
    );
    for range in [
        PageRange::new(VirtAddr::new(0x4000), 1).unwrap(),
        PageRange::new(VirtAddr::new(0x5000), 1).unwrap(),
        PageRange::new(VirtAddr::new(0x4000), 3).unwrap(),
        PageRange::new(VirtAddr::new(0x6000), 1).unwrap(),
    ] {
        assert_eq!(table.unmap(range), Err(MemoryError::NotMapped));
        assert_eq!(table.mappings().collect::<std::vec::Vec<_>>(), before);
    }
    assert_eq!(table.unmap(middle.range()), Ok(middle));
    assert_eq!(
        table.translate(VirtAddr::new(0x4000)),
        Err(MemoryError::NotMapped)
    );
    assert_eq!(table.mappings().collect::<std::vec::Vec<_>>(), [low, high]);
    table.map(middle).unwrap();
    assert_eq!(table.unmap(low.range()), Ok(low));
    assert_eq!(table.unmap(high.range()), Ok(high));
    assert_eq!(table.unmap(middle.range()), Ok(middle));
    assert!(table.is_empty());
    assert_eq!(table.unmap(middle.range()), Err(MemoryError::NotMapped));
    for mapping in [high, middle, low] {
        table.map(mapping).unwrap();
    }
    assert_eq!(
        table.mappings().collect::<std::vec::Vec<_>>(),
        [low, middle, high]
    );
    assert_eq!(
        MappingTable::<0>::new().unmap(low.range()),
        Err(MemoryError::NotMapped)
    );
}

#[test]
fn protect_revalidates_aliases_before_atomic_permission_replacement() {
    let data = PagePermissions::new(true, false, false).unwrap();
    let code = PagePermissions::new(false, true, false).unwrap();
    let ro = PagePermissions::new(false, false, false).unwrap();
    let user = PagePermissions::new(false, false, true).unwrap();
    let range = PageRange::new(VirtAddr::new(0x1000), 2).unwrap();
    let alias_range = PageRange::new(VirtAddr::new(0x5000), 1).unwrap();
    let original =
        Mapping::new(range, PhysAddr::new(0x8000), data, CachePolicy::WriteBack).unwrap();
    let alias = Mapping::new(
        alias_range,
        PhysAddr::new(0x9000),
        data,
        CachePolicy::WriteBack,
    )
    .unwrap();
    let mut table = MappingTable::<2>::new();
    table.map(alias).unwrap();
    table.map(original).unwrap();
    let before = table.mappings().collect::<std::vec::Vec<_>>();
    for permissions in [code, user] {
        assert_eq!(
            table.protect(range, permissions),
            Err(MemoryError::InvalidPermissions)
        );
        assert_eq!(table.mappings().collect::<std::vec::Vec<_>>(), before);
    }
    assert_eq!(
        table.protect(PageRange::new(range.start(), 0).unwrap(), ro),
        Err(MemoryError::InvalidSize)
    );
    for missing in [
        PageRange::new(range.start(), 1).unwrap(),
        PageRange::new(VirtAddr::new(0x2000), 1).unwrap(),
        PageRange::new(VirtAddr::new(0x3000), 1).unwrap(),
    ] {
        assert_eq!(table.protect(missing, ro), Err(MemoryError::NotMapped));
    }
    assert_eq!(table.mappings().collect::<std::vec::Vec<_>>(), before);
    assert_eq!(table.protect(alias_range, ro), Ok(alias));
    assert_eq!(table.protect(range, code), Ok(original));
    let changed = table.lookup(range.start()).unwrap();
    assert_eq!(changed.permissions(), code);
    assert_eq!(changed.physical_start(), original.physical_start());
    assert_eq!(changed.cache(), original.cache());
    assert_eq!(
        table.resolve(VirtAddr::new(0x2fff), AccessType::Execute, false),
        Ok(PhysAddr::new(0x9fff))
    );
    assert_eq!(
        table.resolve(range.start(), AccessType::Write, false),
        Err(MemoryError::PermissionDenied)
    );
    let before = table.mappings().collect::<std::vec::Vec<_>>();
    assert_eq!(
        table.protect(alias_range, data),
        Err(MemoryError::InvalidPermissions)
    );
    assert_eq!(table.mappings().collect::<std::vec::Vec<_>>(), before);
    table.unmap(alias_range).unwrap();
    assert_eq!(table.protect(range, user), Ok(changed));
    assert_eq!(
        table.resolve(range.start(), AccessType::Read, true),
        Ok(PhysAddr::new(0x8000))
    );
    assert_eq!(table.protect(range, code).unwrap().permissions(), user);
    assert_eq!(table.len(), 1);
}

#[test]
fn protect_preserves_kernel_only_nonexecutable_device_mappings() {
    let ro = PagePermissions::new(false, false, false).unwrap();
    let mapping = Mapping::new(
        PageRange::new(VirtAddr::new(0x1000), 1).unwrap(),
        PhysAddr::new(0x9000),
        ro,
        CachePolicy::Device,
    )
    .unwrap();
    let mut table = MappingTable::<1>::new();
    table.map(mapping).unwrap();
    for permissions in [
        PagePermissions::new(false, true, false).unwrap(),
        PagePermissions::new(true, false, true).unwrap(),
    ] {
        assert_eq!(
            table.protect(mapping.range(), permissions),
            Err(MemoryError::InvalidPermissions)
        );
        assert_eq!(table.lookup(mapping.range().start()), Some(mapping));
    }
    let data = PagePermissions::new(true, false, false).unwrap();
    assert_eq!(table.protect(mapping.range(), data), Ok(mapping));
    assert_eq!(
        table.lookup(mapping.range().start()).unwrap().cache(),
        CachePolicy::Device
    );
}

#[test]
fn checked_byte_helpers_validate_all_bounds_before_writing() {
    let mut bytes = [0xa5; 8];
    assert_eq!(zero_bytes(&mut bytes, 2, 3), Ok(()));
    assert_eq!(bytes, [0xa5, 0xa5, 0, 0, 0, 0xa5, 0xa5, 0xa5]);
    let before = bytes;
    for (offset, len, error) in [
        (7, 2, MemoryError::OutOfRange),
        (9, 0, MemoryError::OutOfRange),
        (usize::MAX, 1, MemoryError::Overflow),
    ] {
        assert_eq!(zero_bytes(&mut bytes, offset, len), Err(error));
        assert_eq!(bytes, before);
    }
    assert_eq!(zero_bytes(&mut bytes, 8, 0), Ok(()));
    let source = [1, 2, 3, 4, 5];
    assert_eq!(copy_bytes(&mut bytes, 5, &source, 1, 3), Ok(()));
    assert_eq!(&bytes[5..], &[2, 3, 4]);
    let before = bytes;
    for (dst, src, len, error) in [
        (7, 0, 2, MemoryError::OutOfRange),
        (0, 4, 2, MemoryError::OutOfRange),
        (usize::MAX, 0, 1, MemoryError::Overflow),
        (0, usize::MAX, 1, MemoryError::Overflow),
        (0, 6, 0, MemoryError::OutOfRange),
        (9, 0, 0, MemoryError::OutOfRange),
    ] {
        assert_eq!(copy_bytes(&mut bytes, dst, &source, src, len), Err(error));
        assert_eq!(bytes, before);
    }
    assert_eq!(copy_bytes(&mut bytes, 8, &source, 5, 0), Ok(()));
    assert_eq!(zero_bytes(&mut [], 0, 0), Ok(()));
    assert_eq!(copy_bytes(&mut [], 0, &[], 0, 0), Ok(()));
}

#[test]
fn checked_moves_support_both_overlap_directions_and_failure_atomicity() {
    let mut right = [0, 1, 2, 3, 4, 5, 6, 7];
    assert_eq!(move_bytes(&mut right, 0, 2, 6), Ok(()));
    assert_eq!(right, [0, 1, 0, 1, 2, 3, 4, 5]);
    let mut left = [0, 1, 2, 3, 4, 5, 6, 7];
    assert_eq!(move_bytes(&mut left, 2, 0, 6), Ok(()));
    assert_eq!(left, [2, 3, 4, 5, 6, 7, 6, 7]);
    let before = left;
    assert_eq!(move_bytes(&mut left, 0, 0, 8), Ok(()));
    assert_eq!(move_bytes(&mut left, 8, 8, 0), Ok(()));
    for (src, dst, len, error) in [
        (7, 0, 2, MemoryError::OutOfRange),
        (0, 7, 2, MemoryError::OutOfRange),
        (usize::MAX, 0, 1, MemoryError::Overflow),
        (0, usize::MAX, 1, MemoryError::Overflow),
    ] {
        assert_eq!(move_bytes(&mut left, src, dst, len), Err(error));
        assert_eq!(left, before);
    }
    assert_eq!(move_bytes(&mut [], 0, 0, 0), Ok(()));
}

#[test]
fn runtime_self_test_is_public_repeatable_and_side_effect_free() {
    for _ in 0..32 {
        assert_eq!(self_test(), Ok(()));
    }
}

#[test]
fn top_of_address_space_extents_keep_exclusive_ends_representable() {
    let top = usize::MAX & !(PAGE_SIZE - 1);
    let start = top - PAGE_SIZE;
    let range = PageRange::new(VirtAddr::new(start), 1).unwrap();
    let mut used = [u64::MAX];
    let mut reserved = [u64::MAX];
    let mut frames = BitmapFrames::new(PhysAddr::new(start), 1, &mut used, &mut reserved).unwrap();
    let extent = frames.allocate_contiguous(1).unwrap();
    assert_eq!(extent.end(), PhysAddr::new(top));
    assert_eq!(frames.allocate(), Err(MemoryError::OutOfMemory));
    let mapping = Mapping::new(
        range,
        extent.start(),
        PagePermissions::new(false, true, false).unwrap(),
        CachePolicy::WriteBack,
    )
    .unwrap();
    let mut table = MappingTable::<1>::new();
    table.map(mapping).unwrap();
    assert_eq!(
        table.resolve(VirtAddr::new(top - 1), AccessType::Execute, false),
        Ok(PhysAddr::new(top - 1))
    );
    assert_eq!(
        table.translate(VirtAddr::new(top)),
        Err(MemoryError::NotMapped)
    );
    assert_eq!(
        PageRange::new(VirtAddr::new(top), 1),
        Err(MemoryError::Overflow)
    );
    assert_eq!(
        FrameExtent::new(PhysAddr::new(top), 1),
        Err(MemoryError::Overflow)
    );
    assert_eq!(
        PageRange::covering(VirtAddr::new(top - 1), 1).unwrap(),
        range
    );
    assert_eq!(
        PageRange::covering(VirtAddr::new(top), 1),
        Err(MemoryError::Overflow)
    );
    assert_eq!(table.unmap(range), Ok(mapping));
    assert_eq!(frames.stats().allocated, 1);
    frames.free_extent(extent).unwrap();
    assert_eq!(frames.stats().free, 1);
}

#[test]
fn descriptor_permission_matrix_preserves_privilege_and_device_restrictions() {
    for user in [false, true] {
        for (writable, executable) in [(false, false), (true, false), (false, true)] {
            let permissions = PagePermissions::new(writable, executable, user).unwrap();
            for cache in [
                CachePolicy::WriteBack,
                CachePolicy::NonCacheable,
                CachePolicy::Device,
            ] {
                let descriptor = PageDescriptor::new(
                    PhysAddr::new((1usize << 48) - PAGE_SIZE),
                    permissions,
                    cache,
                );
                if cache == CachePolicy::Device && (user || executable) {
                    assert_eq!(descriptor, Err(MemoryError::InvalidPermissions));
                    continue;
                }
                let descriptor = descriptor.unwrap();
                let bits = descriptor.bits();
                assert_eq!(
                    descriptor.output_address().get(),
                    (1usize << 48) - PAGE_SIZE
                );
                assert_eq!(
                    (bits >> 6) & 3,
                    u64::from(user) | (u64::from(!writable) << 1)
                );
                assert_eq!((bits >> 53) & 1, u64::from(user || !executable));
                assert_eq!((bits >> 54) & 1, u64::from(!user || !executable));
                assert_eq!((bits >> 10) & 1, 1);
                let (index, share) = match cache {
                    CachePolicy::WriteBack => (0, 3),
                    CachePolicy::NonCacheable => (1, 3),
                    CachePolicy::Device => (2, 2),
                };
                assert_eq!((bits >> 2) & 7, index);
                assert_eq!((bits >> 8) & 3, share);
            }
        }
    }
}

#[test]
fn empty_arenas_fail_allocations_without_invalidating_valid_checkpoints() {
    let mut arena = BumpArena::new(&mut []);
    let checkpoint = arena.checkpoint();
    assert_eq!(
        arena
            .allocate(core::alloc::Layout::from_size_align(1, 1).unwrap())
            .err(),
        Some(MemoryError::OutOfMemory)
    );
    assert_eq!(arena.used(), 0);
    assert_eq!(arena.remaining(), 0);
    assert_eq!(arena.rollback(checkpoint), Ok(()));
    assert_eq!(
        arena.rollback(checkpoint),
        Err(MemoryError::InvalidCheckpoint)
    );
}

// NEXT TEST
