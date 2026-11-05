// kernel/src/memory/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MEMORY MODULE — Exo-OS Memory Subsystem Root (Layer 0)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Full hierarchy:
//
//   memory/
//   ├── core/         — types, constants, layout, addresses
//   ├── physical/     — buddy, zones, frame descriptors, SLUB, NUMA-aware
//   ├── virt/         — page tables, VMA, address spaces, fault handler
//   ├── heap/         — global allocator, vmalloc, per-CPU magazine
//   ├── dma/          — DMA channels, IOMMU (VT-d / AMD-Vi), completion
//   ├── swap/         — swap backend, CLOCK eviction policy
//   ├── cow/          — lock-free COW tracker
//   ├── huge_pages/   — 2 MiB THP
//   ├── protection/   — NX, SMEP, SMAP, PKU
//   ├── integrity/    — canary, guard pages, KASAN-lite
//   ├── numa/         — nodes, distances, policy, migration
//   └── utils/        — futex table (UNIQUE), OOM killer, shrinker
//
// Architecture rules (docs/recast/regle_bonus.md):
//   • LAYER 0: no scheduler/process/ipc/fs dependencies.
//   • Lock ordering: Memory → Scheduler → Security → IPC → FS.
//   • RULE IA-KERNEL-01: static .rodata tables only.
//   • RULE EMERGENCY-01: EmergencyPool initialized FIRST.
//   • DmaWakeupHandler trait: defined here, implemented by process/.
//   • FutexTable: UNIQUE SINGLETON in this module.
//
// Initialization order:
//   Phase 1  — physical::allocator init_phase1..4 (buddy + SLUB + NUMA)
//   Phase 2  — virtual address spaces (KERNEL_AS.init() via arch/boot)
//   Phase 3  — heap (#[global_allocator] already active via static)
//   Phase 4  — DMA subsystem
//   Phase 5  — protection (NX/SMEP/SMAP/PKU)
//   Phase 6  — integrity (canary/guard/KASAN)
//   Phase 7  — utils (futex/OOM/shrinker)
//   Phase 8  — numa

#![allow(clippy::module_inception)]

// ─────────────────────────────────────────────────────────────────────────────
// Submodules
// ─────────────────────────────────────────────────────────────────────────────

pub mod arch_iface;
pub mod core;
pub mod physical;
// "virtual" is a reserved Rust keyword: the virtual/ directory is
// declared as public module `virt` via the #[path] attribute.
pub mod cow;
pub mod dma;
pub mod heap;
pub mod huge_pages;
pub mod integrity;
pub mod numa;
pub mod protection;
pub mod swap;
pub mod utils;
#[path = "virtual/mod.rs"]
pub mod virt;

// ─────────────────────────────────────────────────────────────────────────────
// Top-level re-exports (module public API)
// ─────────────────────────────────────────────────────────────────────────────

// Core types — exported first as all others depend on them.
pub use self::core::{
    phys_to_virt, virt_to_phys_physmap, AllocError, AllocFlags, Frame, FrameRange, Page, PageFlags,
    PageRange, PhysAddr, PhysRange, VirtAddr, VirtRange, ZoneType, BUDDY_MAX_ORDER,
    BUDDY_ORDER_COUNT, DMA_RING_SIZE, EMERGENCY_POOL_SIZE, FUTEX_HASH_BUCKETS, HUGE_PAGE_SIZE,
    KERNEL_HEAP_START, PAGE_SIZE, PER_CPU_POOL_SIZE, PHYS_MAP_BASE, VMALLOC_BASE,
};

// Physical allocator — frame allocation API.
pub use physical::{alloc_page, alloc_pages, free_page, free_pages};

// Heap — global allocator (SLUB / vmalloc).
pub use heap::{drain_on_context_switch, drain_on_memory_pressure, heap_alloc, heap_free};

// DMA — wakeup trait injected by process/.
pub use dma::DmaWakeupHandler;

// Swap.
pub use swap::{is_critical, should_swap, SwapDevice, SwapPte, SwapSlot};

// COW.
pub use cow::COW_TRACKER;

// Huge pages (THP 2 MiB).
pub use huge_pages::{alloc_huge_page, free_huge_page, split_huge_page, try_promote_to_huge};

// Hardware protection (NX / SMEP / SMAP / PKU).
pub use protection::{copy_from_user, copy_to_user, nx_page_flags, zero_user};

// Integrity (canary / guard pages / KASAN-lite).
pub use integrity::{
    cpu_canary, kasan_check_access, kasan_on_alloc, kasan_on_free, thread_canary,
    verify_thread_canary,
};

// NUMA — policy from numa::policy (distinct from physical::allocator::NumaPolicy).
pub use numa::policy::NumaPolicy;
pub use numa::{closest_node, numa_distance, NUMA_NODES};

// Utils (UNIQUE futex table + OOM killer + shrinker).
pub use utils::{
    futex_cancel, futex_wait, futex_wake, futex_wake_n, oom_kill, register_oom_candidate_provider,
    register_oom_kill_sender, register_shrinker, run_shrinkers, OomKillCandidate, OomScorer,
    FUTEX_TABLE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Global initialization function
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the entire memory subsystem.
///
/// # Safety
/// - CPL 0.
/// - RULE EMERGENCY-01: kernel must compile with the `emergency_pool`
///   feature active, which pre-initializes EmergencyPool in
///   statics before even entering this function.
/// - `phys_start` / `phys_end`: total physical memory bounds
///   (obtained from BIOS E820 or UEFI GetMemoryMap).
/// - `regions`: [(start_phys, size_bytes)] array of free ranges.
///
/// # Note on Phase 2 (virtual)
/// `virt::address_space::kernel::KERNEL_AS.init(pml4_phys)` must be
/// called separately by arch/ code **after** the boot PML4 has
/// been built and this function has returned.
///
/// # Note on swap-in
/// `virt::fault::swap_in::register_backend_swap_provider()` is called
/// separately by `arch/x86_64/boot/` after `KERNEL_AS.init()`, when
/// the kernel address space is published.
pub unsafe fn init(phys_start: PhysAddr, phys_end: PhysAddr, regions: &[(u64, u64)]) {
    // ── Phase 1: physical allocator ──────────────────────────────────────────
    // Phase 1a — bootstrap bitmap (EmergencyPool activated first)
    physical::init_phase1_bitmap(phys_start, phys_end);
    // Phase 1b — freeing free memory regions (one by one)
    for &(start, size) in regions {
        physical::init_phase2_free_region(
            PhysAddr::new(start),
            PhysAddr::new(start.wrapping_add(size)),
        );
    }
    // Phase 1c — SLUB / slab
    physical::init_phase3_slab_slub();
    cow::init();
    // Phase 1d — NUMA topology (0xFF = all 8 nodes active by default)
    physical::init_phase4_numa(0xFF);

    // ── Phase 2: virtual address spaces ──────────────────────────────────────
    // Delegated to arch/: virt::address_space::kernel::KERNEL_AS.init(pml4_phys)
    // called from arch/x86_64/boot.rs after this function returns.

    // ── Phase 3: global heap ─────────────────────────────────────────────────
    // #[global_allocator] KernelAllocator is already statically linked.
    // It automatically delegates to SLUB (small allocs) and vmalloc (large).

    // ── Phase 4: DMA ─────────────────────────────────────────────────────────
    dma::init();

    // ── Phase 5: hardware protection ─────────────────────────────────────────
    protection::init();

    // ── Phase 6: memory integrity ────────────────────────────────────────────
    integrity::init();

    // ── Phase 7: utilities ───────────────────────────────────────────────────
    utils::init();

    // ── Phase 8: NUMA ────────────────────────────────────────────────────────
    numa::init();
}

/// Allocates an already zeroed physical page.
#[inline]
pub fn alloc_zeroed_page() -> Result<Frame, AllocError> {
    physical::alloc_page(AllocFlags::ZEROED)
}
