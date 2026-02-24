// ipc/shared_memory/mod.rs — Module racine de la mémoire partagée IPC pour Exo-OS
//
// Ce module expose l'API complète de gestion de mémoire partagée :
//   - page      : ShmPage, PageFlags, PhysAddr (blocs élémentaires)
//   - pool      : pool statique de 256 pages pré-allouées, lock-free CAS
//   - descriptor: ShmDescriptor, répertoire global MAX_SHM_REGIONS=1024
//   - mapping   : association région ↔ espace virtuel processus
//   - allocator : allocation par classe de taille (Small/Medium/Large/Huge)
//   - numa_aware: allocation avec affinité NUMA (jusqu'à 8 nœuds)

pub mod page;
pub mod pool;
pub mod descriptor;
pub mod mapping;
pub mod allocator;
pub mod numa_aware;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Primitives de page
pub use page::{
    ShmPage, PageFlags, PhysAddr, ShmPageStats,
    PAGE_SIZE, HUGE_PAGE_SIZE,
};

// Pool de pages
pub use pool::{
    init_shm_pool, shm_page_alloc, shm_page_free,
    shm_page_ref, shm_page_phys, shm_pool_stats,
    shm_alloc_contiguous, shm_free_contiguous,
    ShmPoolStats, POOL_BITMAP_WORDS,
};

// Descripteurs de régions
pub use descriptor::{
    ShmDescriptor, ShmId, ShmPermissions, ShmState,
    ShmDescDirectory, MAX_SHM_PAGES_PER_DESC, MAX_SHM_REGIONS,
    SHM_DESC_DIR, alloc_shm_id,
    shm_create, shm_get_id, shm_get_size, shm_destroy, shm_region_count,
};

// Mapping virtuel
pub use mapping::{
    VirtAddr, ShmMapping, ShmMapResult, MapPageFn, UnmapPageFn,
    MAX_SHM_MAPPINGS,
    register_map_hook, register_unmap_hook,
    shm_map, shm_unmap, shm_mapping_count,
};

// Allocateur par classe de taille
pub use allocator::{
    ShmHandle, ShmSizeClass, ShmAllocatorStats,
    shm_alloc, shm_alloc_pages, shm_free, shm_free_by_idx,
    shm_free_page_count, shm_can_alloc, shm_allocator_stats,
};

// NUMA-aware
pub use numa_aware::{
    NumaManager, NumaNodeId, NumaBitmap, NumaStats, NumaStatsSnapshot,
    MAX_NUMA_NODES, NUMA_PAGES_PER_NODE, NUMA_MANAGER,
    numa_init, numa_shm_alloc, numa_shm_free,
    numa_stats, numa_node_free_pages,
};
