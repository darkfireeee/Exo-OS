// kernel/src/memory/physical/allocator/mod.rs
//
// Module allocator — regroupe tous les allocateurs de pages physiques.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod numa_hints;
pub mod bitmap;
pub mod buddy;
pub mod numa_aware;
pub mod slab;
pub mod slub;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS PRINCIPAUX
// ─────────────────────────────────────────────────────────────────────────────

pub use numa_hints::{
    NumaNode, SizeClass,
    numa_distance, cpu_numa_node,
    set_numa_topology,
};

pub use bitmap::{BitmapAllocator, BOOTSTRAP_BITMAP};

pub use buddy::{
    GlobalBuddyAllocator, BUDDY,
    alloc_pages, free_pages, alloc_page, free_page,
};

pub use numa_aware::{
    NumaPolicy, NumaAllocContext, NumaAllocator, PageAllocator,
    NUMA_ALLOCATOR, NUMA_STATS,
    set_current_policy, get_current_policy,
};

pub use slab::{
    SlabCache, SlabCacheStats, SIZE_CLASSES, N_SIZE_CLASSES, SizeClassInfo,
    size_class_for, SLAB_CACHES,
    alloc as slab_alloc,
    free  as slab_free,
    init_all as slab_init_all,
    register_slab_page_provider, SlabPageProvider,
};

pub use slub::{
    SlubCache, SlubCacheStats, SLUB_CACHES,
    alloc as slub_alloc,
    free  as slub_free,
    init_all as slub_init_all,
};

// ─────────────────────────────────────────────────────────────────────────────
// INIT GLOBALE DES ALLOCATEURS
// ─────────────────────────────────────────────────────────────────────────────

use crate::memory::core::PhysAddr;

/// Ordre d'initialisation des allocateurs (appelé depuis memory::init()).
///
/// 1. EmergencyPool (déjà initialisé avant tout — RÈGLE EMERGENCY-01)
/// 2. BitmapAllocator (early boot)
/// 3. BuddyAllocator  (après init physmap)
/// 4. SlabAllocator   (après buddy)
/// 5. SlubAllocator   (optionnel, remplace slab en production)
/// 6. NumaAllocator   (après topologie ACPI)
pub fn init_phase1_bitmap(phys_start: PhysAddr, phys_end: PhysAddr) {
    // SAFETY: Single-CPU, avant init SMP, appelé une seule fois.
    unsafe { BOOTSTRAP_BITMAP.init(phys_start, phys_end); }
}

pub fn init_phase2_free_region(start: PhysAddr, end: PhysAddr) {
    // SAFETY: Single-CPU, appelé depuis le parser E820/UEFI map.
    unsafe { BOOTSTRAP_BITMAP.add_free_region(start, end); }
}

/// Initialise la zone DMA32 du buddy allocator (couvre la RAM < 4 GiB).
///
/// Doit être appelé APRÈS init_phase1_bitmap+init_phase2_free_region,
/// et AVANT init_phase3_slab_slub. La physmap doit être mappée.
///
/// `bitmap_buf` / `bitmap_words` : buffer statique fourni par l'appelant
/// pour stocker le bitmap de disponibilité (1 bit par page de la zone).
/// Pour couvrir 4 GiB : 16384 u64 × 8 = 128 KiB.
///
/// # Safety
/// - Single-CPU, physmap opérationnelle, appelé une seule fois.
pub unsafe fn init_phase2b_buddy_zone(
    phys_start:   PhysAddr,
    phys_end:     PhysAddr,
    bitmap_buf:   *mut u64,
    bitmap_words: usize,
) {
    use crate::memory::core::ZoneType;
    use crate::memory::core::constants::ZONE_DMA32_END;
    // Clamp à la limite DMA32 (< 4 GiB)
    let dma32_end = PhysAddr::new(phys_end.as_u64().min(ZONE_DMA32_END as u64));
    if phys_start >= dma32_end { return; }
    BUDDY.init_zone(ZoneType::Dma32, phys_start, dma32_end, bitmap_buf, bitmap_words);
    BUDDY.mark_initialized();
}

/// Ajoute une région de RAM libre au buddy allocator (phase 2b).
/// Appeler pour chaque région E820/UEFI utilisable, après init_phase2b_buddy_zone.
///
/// # Safety
/// - Zone buddy initialisée, physmap opérationnelle, single-CPU.
pub unsafe fn init_phase2b_buddy_free_region(start: PhysAddr, end: PhysAddr) {
    BUDDY.add_free_zone_region(start, end);
}

/// Initialise le SLUB/Slab après que le buddy soit opérationnel.
pub fn init_phase3_slab_slub() {
    slab_init_all();
    slub_init_all();
}

pub fn init_phase4_numa(active_nodes_mask: u8) {
    NUMA_ALLOCATOR.init(active_nodes_mask);
}
