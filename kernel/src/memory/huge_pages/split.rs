// kernel/src/memory/huge_pages/split.rs
//
// Huge page split at page-table level — 2 MiB -> 512 x 4 KiB.
// Layer 0 — no external dependencies except `spin`.
//
// This module handles page-table side splits: it reconstructs 4 KiB PTEs
// from a single PDE (huge page) by iterating over the 512 frames.
// Actual physical liberation is in thp.rs.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::{Frame, PhysAddr, HUGE_PAGE_SIZE, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

/// Huge page split statistics.
pub struct SplitStats {
    /// Number of successful splits.
    pub splits_done: AtomicU64,
    /// Number of rejected splits (page not huge or not splittable).
    pub splits_refused: AtomicU64,
    /// Total number of reconstructed PTEs (splits_done x 512).
    pub ptes_rebuilt: AtomicU64,
}

impl Default for SplitStats {
    fn default() -> Self {
        Self::new()
    }
}

impl SplitStats {
    const fn new() -> Self {
        SplitStats {
            splits_done: AtomicU64::new(0),
            splits_refused: AtomicU64::new(0),
            ptes_rebuilt: AtomicU64::new(0),
        }
    }
}

pub static SPLIT_STATS: SplitStats = SplitStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT RESULT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a huge page split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitResult {
    /// Split succeeded — 512 frames returned to buddy.
    Done,
    /// Address not 2 MiB aligned.
    NotAligned,
    /// Page not marked huge in provided PDE.
    NotHugePage,
    /// Failed to allocate intermediate PT table.
    PtAllocFailed,
}

// ─────────────────────────────────────────────────────────────────────────────
// HUGE PAGE PDE DESCRIPTION (2 MiB)
// ─────────────────────────────────────────────────────────────────────────────

/// Flags extracted from a PDE (Page Directory Entry) for a huge page.
///
/// These flags are propagated to the 512 PTEs generated during split.
#[derive(Debug, Clone, Copy)]
pub struct HugePdeFlags {
    /// Write allowed.
    pub writable: bool,
    /// User-mode access allowed.
    pub user: bool,
    /// NX bit active.
    pub no_execute: bool,
    /// Cache disabled (MMIO).
    pub no_cache: bool,
    /// Write-through enabled.
    pub write_through: bool,
    /// Global page (TLB global entry).
    pub global: bool,
    /// COW flag (OS available bit 9).
    pub cow: bool,
}

impl HugePdeFlags {
    /// Reconstructs PDE flags from raw bits of an x86_64 PDE.
    pub fn from_raw_pde(raw: u64) -> Self {
        HugePdeFlags {
            writable: raw & 0x2 != 0,
            user: raw & 0x4 != 0,
            no_execute: raw & (1u64 << 63) != 0,
            no_cache: raw & 0x10 != 0,
            write_through: raw & 0x8 != 0,
            global: raw & 0x100 != 0,
            cow: raw & 0x200 != 0, // bit 9 = COW (OS-defined)
        }
    }

    /// Encodes into 4 KiB PTE flags (PRESENT | inherited flags).
    pub fn to_pte_flags(self) -> u64 {
        let mut f: u64 = 0x1; // PRESENT always
        if self.writable {
            f |= 0x2;
        }
        if self.user {
            f |= 0x4;
        }
        if self.write_through {
            f |= 0x8;
        }
        if self.no_cache {
            f |= 0x10;
        }
        if self.global {
            f |= 0x100;
        }
        if self.cow {
            f |= 0x200;
        }
        if self.no_execute {
            f |= 1u64 << 63;
        }
        f
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 512 PTEs GENERATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Generates the 512 PTE descriptors to replace a huge PDE.
///
/// `huge_phys`: base physical address of huge page (2 MiB aligned).
/// `flags`: flags propagated from PDE.
///
/// Returns an array of 512 (`phys_frame_start`, `pte_raw`) to write
/// into the newly allocated Page Table.
pub fn generate_split_ptes(huge_phys: PhysAddr, flags: HugePdeFlags) -> [u64; 512] {
    let pte_flags = flags.to_pte_flags();
    let base = huge_phys.as_u64();
    let mut ptes = [0u64; 512];
    for i in 0..512usize {
        let frame_phys = base + (i as u64 * PAGE_SIZE as u64);
        ptes[i] = frame_phys | pte_flags;
    }
    ptes
}

// ─────────────────────────────────────────────────────────────────────────────
// LOGICAL SPLIT (without direct page table access)
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a logical split: the 512 (physical, flags) to insert.
pub struct SplitOutput {
    /// Physical address of PT table to create (allocated by caller).
    pub pt_phys: PhysAddr,
    /// The 512 raw PTE entries to write into `pt_phys`.
    pub ptes: [u64; 512],
}

/// Performs logical split of a huge page.
///
/// `huge_phys`: physical address of huge page (2 MiB aligned).
/// `raw_pde`: raw value of original PDE (to extract flags).
/// `pt_frame`: frame allocated by caller for new PT.
///
/// The caller must:
///   1. Allocate a frame (`pt_frame`) for the 512-PTE PT.
///   2. Call this function.
///   3. Write `output.ptes` into frame `pt_frame` via physmap.
///   4. Replace original PDE with a PDE pointing to `pt_frame`
///      (without PSE / huge_page bit).
///   5. Flush TLB for range [huge_phys, huge_phys + 2 MiB].
///
/// # Safety
/// `huge_phys` must be 2 MiB aligned. The huge page frame must
/// remain allocated until PTEs are rewritten (pages remain usable).
pub unsafe fn split_huge_pde(
    huge_phys: PhysAddr,
    raw_pde: u64,
    pt_frame: Frame,
) -> Result<SplitOutput, SplitResult> {
    // Verify 2 MiB alignment.
    if !huge_phys.as_u64().is_multiple_of(HUGE_PAGE_SIZE as u64) {
        SPLIT_STATS.splits_refused.fetch_add(1, Ordering::Relaxed);
        return Err(SplitResult::NotAligned);
    }

    // Verify that this is indeed a huge PDE (bit 7 / PSE).
    if raw_pde & 0x80 == 0 {
        SPLIT_STATS.splits_refused.fetch_add(1, Ordering::Relaxed);
        return Err(SplitResult::NotHugePage);
    }

    // Extract flags from PDE and generate the 512 PTEs.
    let flags = HugePdeFlags::from_raw_pde(raw_pde);
    let ptes = generate_split_ptes(huge_phys, flags);

    SPLIT_STATS.splits_done.fetch_add(1, Ordering::Relaxed);
    SPLIT_STATS.ptes_rebuilt.fetch_add(512, Ordering::Relaxed);

    Ok(SplitOutput {
        pt_phys: pt_frame.start_address(),
        ptes,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT WITH DIRECT WRITE VIA PHYSMAP
// ─────────────────────────────────────────────────────────────────────────────

/// Performs split by directly writing PTEs into the physmap.
///
/// `pde_virt`: virtual address (in physmap) of PDE to replace.
/// `huge_phys`: physical address of huge page.
/// `pt_frame`: frame allocated for new PT.
/// `phys_map_base`: base of physmap (PHYS_MAP_BASE).
///
/// This function atomically writes the 512 PTEs then replaces the PDE.
///
/// # Safety
/// Physmap must be accessible and PDE must point to `huge_phys`.
pub unsafe fn split_huge_pde_in_place(
    pde_virt: *mut u64,
    huge_phys: PhysAddr,
    raw_pde: u64,
    pt_frame: Frame,
    phys_map_base: u64,
) -> SplitResult {
    let output = match split_huge_pde(huge_phys, raw_pde, pt_frame) {
        Ok(o) => o,
        Err(e) => return e,
    };

    // Write the 512 PTEs into the new PT via physmap.
    let pt_virt = (phys_map_base + output.pt_phys.as_u64()) as *mut u64;
    // SAFETY: Physmap is linearly mapped, pt_frame is allocated.
    for i in 0..512usize {
        pt_virt.add(i).write_volatile(output.ptes[i]);
    }

    // Store fence before PDE update.
    core::sync::atomic::fence(Ordering::Release);

    // Replace PDE with non-huge PDE pointing to new PT.
    // Flags: PRESENT | WRITE | (USER if original PDE was user).
    let mut new_pde_flags: u64 = 0x3; // PRESENT + WRITE
    if raw_pde & 0x4 != 0 {
        new_pde_flags |= 0x4;
    } // USER
    if raw_pde & (1u64 << 63) != 0 {
        new_pde_flags |= 1u64 << 63;
    } // NX

    let new_pde = output.pt_phys.as_u64() | new_pde_flags;
    pde_virt.write_volatile(new_pde);

    SplitResult::Done
}
