// kernel/src/memory/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MODULE MEMORY — Racine du sous-système mémoire Exo-OS  (Couche 0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Hiérarchie complète :
//
//   memory/
//   ├── core/         — types, constantes, layout, adresses
//   ├── physical/     — buddy, zones, frame descriptors, SLUB, NUMA-aware
//   ├── virt/         — page tables, VMA, address spaces, fault handler
//   ├── heap/         — allocateur global, vmalloc, per-CPU magazine
//   ├── dma/          — canaux DMA, IOMMU (VT-d / AMD-Vi), completion
//   ├── swap/         — backend swap, politique d'éviction CLOCK
//   ├── cow/          — tracker COW lock-free
//   ├── huge_pages/   — THP 2 MiB
//   ├── protection/   — NX, SMEP, SMAP, PKU
//   ├── integrity/    — canary, guard pages, KASAN-lite
//   ├── numa/         — nœuds, distances, politique, migration
//   └── utils/        — futex table (UNIQUE), OOM killer, shrinker
//
// Règles d'architecture (docs/refonte/regle_bonus.md) :
//   • COUCHE 0 : aucune dépendance scheduler/process/ipc/fs.
//   • Ordonnancement des locks : IPC < Scheduler < Memory < FS.
//   • RÈGLE IA-KERNEL-01 : tables .rodata statiques uniquement.
//   • RÈGLE EMERGENCY-01 : EmergencyPool initialisé EN PREMIER.
//   • DmaWakeupHandler trait : défini ici, implémenté par process/.
//   • FutexTable : SINGLETON UNIQUE dans ce module.
//
// Ordre d'initialisation :
//   Phase 1  — physical::allocator init_phase1..4  (buddy + SLUB + NUMA)
//   Phase 2  — virtual address spaces  (KERNEL_AS.init() via arch/boot)
//   Phase 3  — heap (#[global_allocator] déjà actif via static)
//   Phase 4  — DMA subsystem
//   Phase 5  — protection (NX/SMEP/SMAP/PKU)
//   Phase 6  — integrity (canary/guard/KASAN)
//   Phase 7  — utils (futex/OOM/shrinker)
//   Phase 8  — numa

#![allow(clippy::module_inception)]

// ─────────────────────────────────────────────────────────────────────────────
// Sous-modules
// ─────────────────────────────────────────────────────────────────────────────

pub mod arch_iface;
pub mod core;
pub mod physical;
// "virtual" est un mot-clé réservé Rust : le répertoire virtual/ est
// déclaré comme module public `virt` via l'attribut #[path].
#[path = "virtual/mod.rs"]
pub mod virt;
pub mod heap;
pub mod dma;
pub mod swap;
pub mod cow;
pub mod huge_pages;
pub mod protection;
pub mod integrity;
pub mod numa;
pub mod utils;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports de premier niveau (API publique du module)
// ─────────────────────────────────────────────────────────────────────────────

// Core types — exportés en premier car tous les autres en dépendent.
pub use self::core::{
    PhysAddr, VirtAddr, Frame, Page, PageRange, FrameRange,
    PageFlags, ZoneType, AllocFlags, AllocError,
    PAGE_SIZE, HUGE_PAGE_SIZE, BUDDY_MAX_ORDER, BUDDY_ORDER_COUNT,
    PER_CPU_POOL_SIZE, EMERGENCY_POOL_SIZE, DMA_RING_SIZE, FUTEX_HASH_BUCKETS,
    PhysRange, VirtRange, phys_to_virt, virt_to_phys_physmap,
    PHYS_MAP_BASE, VMALLOC_BASE, KERNEL_HEAP_START,
};

// Physical allocator — API d'allocation frames.
// Note: alloc_zeroed_page n'existe pas dans physical — utiliser
// alloc_page() + écriture manuelle ou heap_alloc_zeroed() pour la heap.
pub use physical::{
    alloc_page, alloc_pages, free_page, free_pages,
};

// Heap — allocateur global (SLUB / vmalloc).
pub use heap::{
    heap_alloc, heap_free,
    drain_on_context_switch, drain_on_memory_pressure,
};

// DMA — trait wakeup injecté par process/.
pub use dma::DmaWakeupHandler;

// Swap.
pub use swap::{SwapDevice, SwapSlot, SwapPte, should_swap, is_critical};

// COW.
pub use cow::COW_TRACKER;

// Huge pages (THP 2 MiB).
pub use huge_pages::{alloc_huge_page, free_huge_page, split_huge_page, try_promote_to_huge};

// Protection matérielle (NX / SMEP / SMAP / PKU).
pub use protection::{copy_from_user, copy_to_user, zero_user, nx_page_flags};

// Intégrité (canary / guard pages / KASAN-lite).
pub use integrity::{
    kasan_on_alloc, kasan_on_free, kasan_check_access,
    cpu_canary, thread_canary, verify_thread_canary,
};

// NUMA — politique depuis numa::policy (distinct de physical::allocator::NumaPolicy).
pub use numa::{NUMA_NODES, numa_distance, closest_node};
pub use numa::policy::NumaPolicy;

// Utils (UNIQUE table futex + OOM killer + shrinker).
pub use utils::{
    FUTEX_TABLE, futex_wait, futex_wake, futex_wake_n, futex_cancel,
    OomScorer, register_oom_kill_sender, oom_kill,
    register_shrinker, run_shrinkers,
};

// ─────────────────────────────────────────────────────────────────────────────
// Fonction d'initialisation globale
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise l'intégralité du sous-système mémoire.
///
/// # Safety
/// - CPL 0.
/// - RÈGLE EMERGENCY-01 : le kernel doit compiler avec le feature
///   `emergency_pool` actif, qui pré-initialise l'EmergencyPool dans
///   les statics avant même d'entrer dans cette fonction.
/// - `phys_start` / `phys_end` : bornes de la mémoire physique totale
///   (obtenues depuis BIOS E820 ou UEFI GetMemoryMap).
/// - `regions` : tableau [(start_phys, size_bytes)] des plages libres.
///
/// # Note sur la phase 2 (virtual)
/// `virt::address_space::kernel::KERNEL_AS.init(pml4_phys)` doit être
/// appelé séparément par le code arch/ **après** que la PML4 de boot a
/// été construite et que cette fonction a retourné.
pub unsafe fn init(phys_start: PhysAddr, phys_end: PhysAddr, regions: &[(u64, u64)]) {
    // ── Phase 1 : allocateur physique ────────────────────────────────────────
    // Phase 1a — bitmap de démarrage (EmergencyPool activé en premier)
    physical::init_phase1_bitmap(phys_start, phys_end);
    // Phase 1b — libération des régions mémoire libres (une par une)
    for &(start, size) in regions {
        physical::init_phase2_free_region(
            PhysAddr::new(start),
            PhysAddr::new(start.wrapping_add(size)),
        );
    }
    // Phase 1c — SLUB / slab
    physical::init_phase3_slab_slub();
    // Phase 1d — topologie NUMA (0xFF = tous les 8 nœuds actifs par défaut)
    physical::init_phase4_numa(0xFF);

    // ── Phase 2 : espaces d'adressage virtuels ────────────────────────────────
    // Délégué à l'arch/ : virt::address_space::kernel::KERNEL_AS.init(pml4_phys)
    // appelé depuis arch/x86_64/boot.rs après retour de cette fonction.

    // ── Phase 3 : heap global ────────────────────────────────────────────────
    // Le #[global_allocator] KernelAllocator est déjà lié statiquement.
    // Il délègue automatiquement à SLUB (petites allocs) et vmalloc (grandes).

    // ── Phase 4 : DMA ─────────────────────────────────────────────────────────
    dma::init();

    // ── Phase 5 : protection matérielle ──────────────────────────────────────
    protection::init();

    // ── Phase 6 : intégrité mémoire ──────────────────────────────────────────
    integrity::init();

    // ── Phase 7 : utilitaires ────────────────────────────────────────────────
    utils::init();

    // ── Phase 8 : NUMA ───────────────────────────────────────────────────────
    numa::init();
}
