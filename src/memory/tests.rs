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

// NEXT TEST
