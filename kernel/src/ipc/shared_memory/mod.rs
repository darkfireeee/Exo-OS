// ipc/shared_memory/mod.rs — Module racine de la mémoire partagée IPC pour Exo-OS
//
// Ce module expose l'API complète de gestion de mémoire partagée :
//   - page      : ShmPage, PageFlags, PhysAddr (blocs élémentaires)
//   - pool      : pool statique de 256 pages pré-allouées, lock-free CAS
//   - descriptor: ShmDescriptor, répertoire global MAX_SHM_REGIONS=1024
//   - mapping   : association région ↔ espace virtuel processus
//   - allocator : allocation par classe de taille (Small/Medium/Large/Huge)
//   - numa_aware: allocation avec affinité NUMA (jusqu'à 8 nœuds)

pub mod allocator;
pub mod descriptor;
pub mod mapping;
pub mod numa_aware;
pub mod page;
pub mod pool;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Primitives de page
pub use page::{PageFlags, PhysAddr, ShmPage, ShmPageStats, HUGE_PAGE_SIZE, PAGE_SIZE};

// Pool de pages
pub use pool::{
    init_shm_pool, shm_alloc_contiguous, shm_free_contiguous, shm_page_alloc, shm_page_free,
    shm_page_phys, shm_page_ref, shm_pool_stats, ShmPoolStats, POOL_BITMAP_WORDS,
};

// Descripteurs de régions
pub use descriptor::{
    alloc_shm_id, shm_create, shm_destroy, shm_get_id, shm_get_size, shm_region_count,
    ShmDescDirectory, ShmDescriptor, ShmId, ShmPermissions, ShmState, MAX_SHM_PAGES_PER_DESC,
    MAX_SHM_REGIONS, SHM_DESC_DIR,
};

// Mapping virtuel
pub use mapping::{
    register_map_hook, register_unmap_hook, shm_map, shm_mapping_count, shm_unmap, MapPageFn,
    ShmMapResult, ShmMapping, UnmapPageFn, VirtAddr, MAX_SHM_MAPPINGS,
};

// Allocateur par classe de taille
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
