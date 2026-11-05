// kernel/src/memory/arch_iface.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MEMORY ↔ ARCHITECTURE INTERFACE — memory/ side
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module exposes registration hooks that architecture (arch/) code
// uses to attach to the memory subsystem during startup.
//
// ## Role
//
// - Converts physical memory map provided by arch/ boot into calls
//   to memory/ allocators (buddy, slab, NUMA).
// - Provides `PhysMemoryRegion` and `PhysRegionType` types that arch/ boot
//   uses to describe physical RAM uniformly (E820 ≡ UEFI).
//
// ## Integration points
//
// `arch/x86_64/boot/memory_map.rs` -> `memory::arch_iface::init_from_regions()`
// `arch/x86_64/memory_iface.rs`    -> `memory::virt::address_space::tlb::register_tlb_ipi_sender()`
//
// ## Architecture rules (DOC2)
//
//   MEM-01: memory/ is LAYER 0 — does NOT depend on scheduler/, process/, etc.
//   MEM-02: EmergencyPool initialized FIRST (before this call).
//   MEM-04: free_pages() is NEVER called before complete TLB shootdown.

// ─────────────────────────────────────────────────────────────────────────────
// INTEGRATION CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// TLB shootdown IPI vector (0xF2) — must match arch/x86_64/idt.
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF2;

/// Reschedule IPI vector (0xF1).
pub const IPI_RESCHEDULE_VECTOR: u8 = 0xF1;

/// Maximum supported NUMA nodes.
pub const MAX_NUMA_NODES: usize = 8;

/// Maximum supported CPUs.
pub const MAX_CPUS: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// MEMORY REGION TYPES
// ─────────────────────────────────────────────────────────────────────────────

/// Describes a physical memory region provided by arch/ at boot.
///
/// Constructed by `arch/x86_64/boot/memory_map.rs` from E820 (Multiboot2)
/// or UEFI Memory Map table. Passed to `init_from_regions()`.
#[derive(Debug, Clone, Copy)]
pub struct PhysMemoryRegion {
    /// Physical start address (aligned to PAGE_SIZE).
    pub base: u64,
    /// Size in bytes (multiple of PAGE_SIZE).
    pub size: u64,
    /// Region type (usable, reserved, acpi...).
    pub region_type: PhysRegionType,
}

impl PhysMemoryRegion {
    /// Exclusive physical end address.
    #[inline]
    pub fn end(&self) -> u64 {
        self.base.wrapping_add(self.size)
    }

    /// Returns `true` if this is usable RAM for the buddy allocator.
    #[inline]
    pub fn is_usable(&self) -> bool {
        matches!(self.region_type, PhysRegionType::Usable)
    }

    /// Returns `true` if this is an ACPI reclaimable region.
    #[inline]
    pub fn is_acpi_reclaimable(&self) -> bool {
        matches!(self.region_type, PhysRegionType::AcpiReclaimable)
    }
}

/// Physical memory region type (fusion of E820 and UEFI types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysRegionType {
    /// Conventional RAM — reclaimable by buddy allocator.
    Usable,
    /// Reserved by firmware / hardware — DO NOT touch.
    Reserved,
    /// ACPI reclaimable — reclaimable after parsing ACPI tables.
    AcpiReclaimable,
    /// ACPI NVS — never free or modify.
    AcpiNvs,
    /// Defective memory (ECC error, BIOS marked bad).
    Defective,
    /// Firmware runtime (UEFI runtime services) — do not touch.
    FirmwareReserved,
}

// ─────────────────────────────────────────────────────────────────────────────
// PHYSICAL MEMORY INITIALIZATION FROM ARCH MAP
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the physical memory subsystem from regions
/// detected by the bootloader (E820 / UEFI).
///
/// This function is the unified entry point that `arch/x86_64/boot/memory_map.rs`
/// or UEFI initialization calls after parsing the memory map.
///
/// ## Sequence (rule MEM-02 DOC2)
///
/// 1. Detect `phys_start` / `phys_end` from usable regions.
/// 2. `physical::allocator::init_phase1_bitmap(phys_start, phys_end)`.
/// 3. For each usable region: `init_phase2_free_region(start, end)`.
/// 4. `init_phase3_slab_slub()`.
/// 5. `init_phase4_numa(active_mask)`.
///
/// ## Prerequisites
/// - `EmergencyPool` must have been initialized BEFORE this call.
/// - Called ONCE ONLY, from the BSP, before AP startup.
/// - `regions` must NOT be empty.
///
/// # Safety
/// - Non-concurrent memory (single CPU at boot).
/// - Ranges in `regions` must not overlap.
/// - Usable addresses must be genuine physical RAM.
pub unsafe fn init_from_regions(regions: &[PhysMemoryRegion]) {
    use crate::memory::core::{PhysAddr, PAGE_SIZE};
    use crate::memory::physical::allocator::{
        init_phase1_bitmap, init_phase2_free_region, init_phase3_slab_slub, init_phase4_numa,
    };

    let page = PAGE_SIZE as u64;

    // ── Detect total physical range ───────────────────────────────────────────
    let mut phys_start = u64::MAX;
    let mut phys_end = 0u64;

    for region in regions {
        if region.size == 0 {
            continue;
        }
        let base = region.base;
        let end = region.end();
        if base < phys_start {
            phys_start = base;
        }
        if end > phys_end {
            phys_end = end;
        }
    }

    if phys_start == u64::MAX || phys_end == 0 || phys_start >= phys_end {
        // No region detected — unrecoverable situation
        return;
    }

    // Align total bounds to PAGE_SIZE
    let ps = PhysAddr::new((phys_start + page - 1) & !(page - 1));
    let pe = PhysAddr::new(phys_end & !(page - 1));

    // ── Phase 1: global bitmap across entire detected range ───────────────────
    init_phase1_bitmap(ps, pe);

    // ── Phase 2: free pages in usable regions ─────────────────────────────────
    for region in regions {
        if !region.is_usable() {
            continue;
        }
        if region.size == 0 {
            continue;
        }

        let base_adj = (region.base + page - 1) & !(page - 1);
        let end_adj = region.end() & !(page - 1);
        if base_adj >= end_adj {
            continue;
        }

        init_phase2_free_region(PhysAddr::new(base_adj), PhysAddr::new(end_adj));
    }

    // ── Phase 3: slab / slub — object allocation caches ───────────────────────
    init_phase3_slab_slub();

    // ── Phase 4: NUMA — node 0 active by default ──────────────────────────────
    // Actual topology will be loaded from ACPI/MADT following this sequence.
    init_phase4_numa(0b0000_0001);
}
