use super::*;
use core::alloc::Layout;

fn require(condition: bool) -> Result<(), MemoryError> {
    if condition {
        Ok(())
    } else {
        Err(MemoryError::SelfTestFailed)
    }
}

/// Run a deterministic, allocation-free memory-service smoke test at runtime.
///
/// All bitmaps, byte storage, region records, and software mappings are local.
/// Physical addresses are descriptions only: no RAM, MMIO, MMU, TLB, or global
/// allocator state is accessed. This may be called before enabling the MMU,
/// provided ordinary Rust stack memory is available. It does not test hardware.
/// Unexpected invariant results return `SelfTestFailed`; operation errors are
/// propagated. Success leaves no persistent state and repeated calls are valid.
pub fn self_test() -> Result<(), MemoryError> {
    frame_mapping_test()?;
    arena_byte_test()
}

fn frame_mapping_test() -> Result<(), MemoryError> {
    let ram = RegionAttributes::new(MemoryKind::Usable, CachePolicy::WriteBack)?;
    let device = RegionAttributes::new(MemoryKind::Device, CachePolicy::Device)?;
    let base = PhysAddr::new(PAGE_SIZE);
    let mut regions = RegionMap::<2>::new();
    regions.insert(MemoryRegion::new(base, 70 * PAGE_SIZE, ram)?)?;
    regions.insert(MemoryRegion::new(
        PhysAddr::new(0x1000_0000),
        PAGE_SIZE,
        device,
    )?)?;
    require(regions.usable_bytes() == 70 * PAGE_SIZE)?;

    let mut used = [0u64; 2];
    let mut reserved = [0u64; 2];
    let mut frames = BitmapFrames::new(base, 70, &mut used, &mut reserved)?;
    frames.reserve(base, 63)?;
    let extent = frames.allocate_aligned(4, 4 * PAGE_SIZE)?;
    require(extent.start() == PhysAddr::new(64 * PAGE_SIZE))?;
    require(regions.lookup(extent.start()).map(|r| r.attributes()) == Some(ram))?;
    let stats = frames.stats();
    require(frames.reserve(extent.start(), 1) == Err(MemoryError::AlreadyUsed))?;
    require(frames.stats() == stats)?;

    let data = PagePermissions::new(true, false, false)?;
    let code = PagePermissions::new(false, true, false)?;
    let ro = PagePermissions::new(false, false, false)?;
    let range = PageRange::new(VirtAddr::new(0x10000), extent.frame_count())?;
    let mapping = Mapping::new(range, extent.start(), data, ram.cache())?;
    let mut table = MappingTable::<2>::new();
    table.map(mapping)?;
    require(
        table.resolve(range.end().checked_sub(1)?, AccessType::Write, false)?
            == extent.end().checked_sub(1)?,
    )?;
    let alias_range = PageRange::new(VirtAddr::new(0x20000), 1)?;
    let alias = Mapping::new(alias_range, extent.start(), ro, CachePolicy::WriteBack)?;
    let wrong_cache = Mapping::new(alias_range, extent.start(), ro, CachePolicy::NonCacheable)?;
    require(table.map(wrong_cache) == Err(MemoryError::InvalidAttributes))?;
    require(table.len() == 1)?;
    table.map(alias)?;
    // A read-only alias is compatible with either W or X, but not a user split.
    let user = PagePermissions::new(false, false, true)?;
    require(table.protect(range, user) == Err(MemoryError::InvalidPermissions))?;
    require(table.lookup(range.start()) == Some(mapping))?;
    require(table.protect(range, code)? == mapping)?;
    require(table.resolve(range.start(), AccessType::Execute, false)? == extent.start())?;
    require(
        table.resolve(range.start(), AccessType::Write, false)
            == Err(MemoryError::PermissionDenied),
    )?;
    require(table.protect(alias_range, data) == Err(MemoryError::InvalidPermissions))?;
    let descriptor = PageDescriptor::new(extent.start(), code, CachePolicy::WriteBack)?;
    require(descriptor.output_address() == extent.start())?;
    require(TableDescriptor::new(extent.start())?.bits() == (extent.start().get() as u64 | 3))?;
    require(table.unmap(alias_range)? == alias)?;
    table.unmap(range)?;
    require(table.is_empty() && frames.stats().allocated == 4)?;
    frames.free_extent(extent)?;
    require(
        frames.stats()
            == FrameStats {
                total: 70,
                allocated: 0,
                reserved: 63,
                free: 7,
                largest_free_run: 7,
            },
    )?;
    Ok(())
}

fn arena_byte_test() -> Result<(), MemoryError> {
    let mut backing = [0xa5; 96];
    let mut arena = BumpArena::new(&mut backing);
    let checkpoint = arena.checkpoint();
    let block = arena
        .allocate(Layout::from_size_align(16, 16).map_err(|_| MemoryError::InvalidAlignment)?)?;
    require(block.as_ptr() as usize % 16 == 0)?;
    copy_bytes(block, 0, &[1, 2, 3, 4], 0, 4)?;
    zero_bytes(block, 4, 12)?;
    move_bytes(block, 0, 8, 4)?;
    require(block[8..12] == [1, 2, 3, 4])?;
    require(block[4..8].iter().all(|byte| *byte == 0))?;
    let end = arena.used();
    arena.rollback(checkpoint)?;
    require(arena.used() == 0)?;
    require(arena.contents()[..end].iter().all(|byte| *byte == 0))?;
    require(arena.contents()[end..].iter().all(|byte| *byte == 0xa5))?;
    require(arena.rollback(checkpoint) == Err(MemoryError::InvalidCheckpoint))
}
