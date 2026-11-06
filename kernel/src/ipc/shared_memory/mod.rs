// ipc/shared_memory/mod.rs — Root module for IPC shared memory in Exo-OS
//
// This module exposes the complete shared memory management API:
//   - page      : ShmPage, PageFlags, PhysAddr (elementary blocks)
//   - pool      : static pool of 256 pre-allocated pages, lock-free CAS
//   - descriptor: ShmDescriptor, global directory MAX_SHM_REGIONS=1024
//   - mapping   : region <-> process virtual space association
//   - allocator : size-class allocation (Small/Medium/Large/Huge)
//   - numa_aware: NUMA-affinity allocation (up to 8 nodes)

pub mod allocator;
pub mod descriptor;
pub mod mapping;
pub mod memory_bridge;
pub mod numa_aware;
pub mod page;
pub mod pool;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Page primitives
pub use page::{PageFlags, PhysAddr, ShmPage, ShmPageStats, HUGE_PAGE_SIZE, PAGE_SIZE};

// Page pool
pub use pool::{
    init_shm_pool, shm_alloc_contiguous, shm_free_contiguous, shm_page_alloc, shm_page_free,
    shm_page_phys, shm_page_ref, shm_pool_stats, ShmPoolStats, POOL_BITMAP_WORDS,
};

// Region descriptors
pub use descriptor::{
    alloc_shm_id, shm_create, shm_destroy, shm_get_id, shm_get_size, shm_region_count,
    ShmDescDirectory, ShmDescriptor, ShmId, ShmPermissions, ShmState, MAX_SHM_PAGES_PER_DESC,
    MAX_SHM_REGIONS, SHM_DESC_DIR,
};

// Virtual mapping
pub use mapping::{
    register_map_hook, register_unmap_hook, shm_map, shm_mapping_count, shm_unmap,
    shm_unmap_for_process, MapPageFn, ShmMapResult, ShmMapping, UnmapPageFn, VirtAddr,
    MAX_SHM_MAPPINGS,
};

// Size-class allocator
pub use allocator::{
    shm_alloc, shm_alloc_pages, shm_allocator_stats, shm_can_alloc, shm_free, shm_free_by_idx,
    shm_free_page_count, ShmAllocatorStats, ShmHandle, ShmSizeClass,
};

// NUMA-aware
pub use numa_aware::{
    numa_init, numa_node_free_pages, numa_shm_alloc, numa_shm_free, numa_stats, NumaBitmap,
    NumaManager, NumaNodeId, NumaStats, NumaStatsSnapshot, MAX_NUMA_NODES, NUMA_MANAGER,
    NUMA_PAGES_PER_NODE,
};
