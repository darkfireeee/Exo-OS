// kernel/src/memory/physical/mod.rs
//
// Module physical — gestion de la mémoire physique.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod allocator;
pub mod frame;
pub mod numa;
pub mod stats;
pub mod zone;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS
// ─────────────────────────────────────────────────────────────────────────────

// Frames
pub use frame::{
    AtomicRefCount, EmergencyPool, EmergencyPoolStats, FrameDesc, FrameDescEntry,
    FrameDescriptorTable, FrameFlags, PerCpuFramePool, PerCpuPoolStats, PerCpuPoolTable,
    RefCountDecResult, WaitNode, EMERGENCY_POOL, FRAME_DESCRIPTORS, MAX_PHYS_FRAMES, PER_CPU_POOLS,
};

// Zones
pub use zone::{
    addr_satisfies_flags, zone_for_flags, Dma32Zone, DmaZone, HighZone, MovableZone, NormalZone,
    ZoneDescriptor, ZoneStats,
};

// Allocateurs
pub use allocator::{
    alloc_page, alloc_pages, free_page, free_pages, init_phase1_bitmap, init_phase2_free_region,
    init_phase3_slab_slub, init_phase4_numa, slab_alloc, slab_free, slab_init_all, slub_alloc,
    slub_free, slub_init_all, BitmapAllocator, GlobalBuddyAllocator, NumaAllocContext, NumaNode,
    NumaPolicy, SizeClass, BOOTSTRAP_BITMAP, BUDDY, NUMA_ALLOCATOR, NUMA_STATS, SLAB_CACHES,
    SLUB_CACHES,
};
