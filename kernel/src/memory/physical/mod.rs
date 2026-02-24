// kernel/src/memory/physical/mod.rs
//
// Module physical — gestion de la mémoire physique.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod allocator;
pub mod frame;
pub mod zone;
pub mod numa;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS
// ─────────────────────────────────────────────────────────────────────────────

// Frames
pub use frame::{
    EmergencyPool, EmergencyPoolStats, WaitNode, EMERGENCY_POOL,
    FrameDesc, FrameFlags,
    FrameDescEntry, FrameDescriptorTable, FRAME_DESCRIPTORS, MAX_PHYS_FRAMES,
    AtomicRefCount, RefCountDecResult,
    PerCpuFramePool, PerCpuPoolTable, PER_CPU_POOLS,
    PerCpuPoolStats,
};

// Zones
pub use zone::{
    ZoneDescriptor, ZoneStats,
    zone_for_flags, addr_satisfies_flags,
    DmaZone, Dma32Zone, NormalZone, HighZone, MovableZone,
};

// Allocateurs
pub use allocator::{
    BOOTSTRAP_BITMAP, BitmapAllocator,
    BUDDY, GlobalBuddyAllocator,
    alloc_pages, free_pages, alloc_page, free_page,
    SLAB_CACHES, slab_alloc, slab_free, slab_init_all,
    SLUB_CACHES, slub_alloc, slub_free, slub_init_all,
    NUMA_ALLOCATOR, NUMA_STATS, NumaPolicy, NumaAllocContext,
    NumaNode, SizeClass, hint_numa_node,
    init_phase1_bitmap, init_phase2_free_region,
    init_phase3_slab_slub, init_phase4_numa,
};
