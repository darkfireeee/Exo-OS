// kernel/src/memory/physical/allocator/mod.rs
//
// Allocator module — aggregates all physical page allocators.
// Layer 0 — no external dependencies except `spin`.

pub mod bitmap;
pub mod buddy;
pub mod numa_aware;
pub mod numa_hints;
pub mod slab;
pub mod slub;

// ─────────────────────────────────────────────────────────────────────────────
// MAIN RE-EXPORTS
// ─────────────────────────────────────────────────────────────────────────────

pub use numa_hints::{cpu_numa_node, numa_distance, set_numa_topology, NumaNode, SizeClass};

pub use bitmap::{BitmapAllocator, BOOTSTRAP_BITMAP};

pub use buddy::{alloc_page, alloc_pages, free_page, free_pages, GlobalBuddyAllocator, BUDDY};

pub use numa_aware::{
    get_current_policy, set_current_policy, NumaAllocContext, NumaAllocator, NumaPolicy,
    PageAllocator, NUMA_ALLOCATOR, NUMA_STATS,
};

pub use slab::{
    alloc as slab_alloc, free as slab_free, init_all as slab_init_all, register_slab_page_provider,
    size_class_for, SizeClassInfo, SlabCache, SlabCacheStats, SlabPageProvider, N_SIZE_CLASSES,
    SIZE_CLASSES, SLAB_CACHES,
};

pub use slub::{
    alloc as slub_alloc, free as slub_free, init_all as slub_init_all, SlubCache, SlubCacheStats,
    SLUB_CACHES,
};

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL ALLOCATOR INIT
// ─────────────────────────────────────────────────────────────────────────────

use crate::memory::core::{Frame, PhysAddr, PAGE_SIZE};

/// Initialization order of allocators (called from memory::init()).
///
/// 1. EmergencyPool (already initialized before anything else — EMERGENCY-01 RULE)
/// 2. BitmapAllocator (early boot)
/// 3. BuddyAllocator  (after physmap init)
/// 4. SlabAllocator   (after buddy)
/// 5. SlubAllocator   (optional, replaces slab in production)
/// 6. NumaAllocator   (after ACPI topology)
pub fn init_phase1_bitmap(phys_start: PhysAddr, phys_end: PhysAddr) {
    // SAFETY: Single-CPU, before SMP init, called only once.
    unsafe {
        BOOTSTRAP_BITMAP.init(phys_start, phys_end);
    }
}

pub fn init_phase2_free_region(start: PhysAddr, end: PhysAddr) {
    // SAFETY: Single-CPU, called from E820/UEFI map parser.
    unsafe {
        BOOTSTRAP_BITMAP.add_free_region(start, end);
    }
}

/// Initializes DMA32 zone of buddy allocator (covers RAM < 4 GiB).
///
/// Must be called AFTER init_phase1_bitmap+init_phase2_free_region,
/// and BEFORE init_phase3_slab_slub. Physmap must be mapped.
///
/// `bitmap_buf` / `bitmap_words`: static buffer provided by caller
/// to store availability bitmap (1 bit per zone page).
/// To cover 4 GiB: 16384 u64 × 8 = 128 KiB.
///
/// # Safety
/// - Single-CPU, operational physmap, called only once.
pub unsafe fn init_phase2b_buddy_zone(
    phys_start: PhysAddr,
    phys_end: PhysAddr,
    bitmap_buf: *mut u64,
    bitmap_words: usize,
) {
    use crate::memory::core::constants::ZONE_DMA32_END;
    use crate::memory::core::ZoneType;
    // Clamp to DMA32 limit (< 4 GiB)
    let dma32_end = PhysAddr::new(phys_end.as_u64().min(ZONE_DMA32_END as u64));
    if phys_start >= dma32_end {
        return;
    }
    BUDDY.init_zone(
        ZoneType::Dma32,
        phys_start,
        dma32_end,
        bitmap_buf,
        bitmap_words,
    );
    BUDDY.mark_initialized();
}

/// Adds a free RAM region to buddy allocator (phase 2b).
/// Call for each usable E820/UEFI region, after init_phase2b_buddy_zone.
///
/// # Safety
/// - Buddy zone initialized, physmap operational, single-CPU.
pub unsafe fn init_phase2b_buddy_free_region(start: PhysAddr, end: PhysAddr) {
    let mut cursor = start.as_u64();
    let end = end.as_u64();
    let mut run_start: Option<u64> = None;

    while cursor < end {
        let frame = Frame::containing(PhysAddr::new(cursor));
        let free_for_buddy = BOOTSTRAP_BITMAP.tracked_is_free(frame).unwrap_or(true);

        match (run_start, free_for_buddy) {
            (None, true) => run_start = Some(cursor),
            (Some(s), false) => {
                BUDDY.add_free_zone_region(PhysAddr::new(s), PhysAddr::new(cursor));
                run_start = None;
            }
            _ => {}
        }

        cursor = cursor.saturating_add(PAGE_SIZE as u64);
    }

    if let Some(s) = run_start {
        BUDDY.add_free_zone_region(PhysAddr::new(s), PhysAddr::new(end));
    }
}

/// Initializes SLUB/Slab after buddy is operational.
pub fn init_phase3_slab_slub() {
    slab_init_all();
    slub_init_all();
}

pub fn init_phase4_numa(active_nodes_mask: u8) {
    NUMA_ALLOCATOR.init(active_nodes_mask);
}
