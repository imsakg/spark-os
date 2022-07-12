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

// NEXT TEST
