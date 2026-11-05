// kernel/src/memory/integrity/guard_pages.rs
//
// Guard pages — non-present pages inserted around stacks and sensitive
// heap regions to detect overflows at the time of access.
//
// Architecture:
//   - Each kernel CPU stack: 1 LOW guard page (underflow) + 1 HIGH
//     guard page (overflow).
//   - Each vmalloc allocation > VMALLOC_GUARD_THRESHOLD: a guard page
//     surrounds the region.
//   - A guard page is a PRESENT=0 PTE with a special tag in reserved
//     bits (bits 11:9 = 0b111) to distinguish it from an unmapped page.
//
// Detection: the fault handler calls `is_guard_page_fault(virt)` to
//            decide if the #PF is a guard violation -> kernel panic.
//
// LAYER 0 — no dependency on scheduler/process/ipc/fs.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Tag in bits 11:9 of a PTE marked guard.
/// bits 11:9 = 0b111 -> value 7 shifted by 9 bits.
pub const GUARD_PTE_TAG: u64 = 0b111 << 9;
/// Mask for bits 11:9.
const GUARD_PTE_MASK: u64 = 0b111 << 9;
/// A guard page entry: PRESENT=0, tag=0b111. Must be non-present.
pub const GUARD_PTE_VALUE: u64 = GUARD_PTE_TAG;

/// Vmalloc threshold above which guard pages are inserted.
pub const VMALLOC_GUARD_THRESHOLD: usize = 4 * PAGE_SIZE;

/// Maximum number of tracked guard regions.
const MAX_GUARD_REGIONS: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct GuardPageStats {
    pub regions_registered: AtomicU64,
    pub regions_unregistered: AtomicU64,
    pub violations_detected: AtomicU64,
    pub stack_guards_placed: AtomicU64,
    pub vmalloc_guards_placed: AtomicU64,
    pub false_positives: AtomicU64,
}

impl Default for GuardPageStats {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardPageStats {
    const fn new() -> Self {
        Self {
            regions_registered: AtomicU64::new(0),
            regions_unregistered: AtomicU64::new(0),
            violations_detected: AtomicU64::new(0),
            stack_guards_placed: AtomicU64::new(0),
            vmalloc_guards_placed: AtomicU64::new(0),
            false_positives: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for GuardPageStats {}
pub static GUARD_STATS: GuardPageStats = GuardPageStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Guard Region Directory
// ─────────────────────────────────────────────────────────────────────────────

/// Identifier for a guard region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardRegionId(pub u32);

/// Type of guarded region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRegionKind {
    /// CPU stack (kernel).
    KernelStack { cpu_id: u32 },
    /// Dedicated kernel thread stack.
    KernelThreadStack { tid: u64 },
    /// User thread stack.
    UserStack { tid: u64 },
    /// Vmalloc allocation.
    Vmalloc,
    /// Generic region (heap, MMIO zone, etc.).
    Generic,
}

/// Descriptor for a guard region.
#[derive(Debug, Clone, Copy)]
pub struct GuardRegion {
    /// Virtual start address of lower guard page.
    pub low_guard: u64,
    /// Virtual start address of upper guard page.
    pub high_guard: u64,
    /// Address of protected region (for diagnostics).
    pub region_start: u64,
    pub region_size: u64,
    pub kind: GuardRegionKind,
    pub active: bool,
}

impl GuardRegion {
    #[allow(dead_code)]
    const fn inactive() -> Self {
        Self {
            low_guard: 0,
            high_guard: 0,
            region_start: 0,
            region_size: 0,
            kind: GuardRegionKind::Generic,
            active: false,
        }
    }
}

struct GuardRegionTable {
    regions: [GuardRegion; MAX_GUARD_REGIONS],
    count: usize,
}

impl Default for GuardRegionTable {
    fn default() -> Self {
        Self::new()
    }
}

impl GuardRegionTable {
    const fn new() -> Self {
        Self {
            regions: [GuardRegion {
                low_guard: 0,
                high_guard: 0,
                region_start: 0,
                region_size: 0,
                kind: GuardRegionKind::Generic,
                active: false,
            }; MAX_GUARD_REGIONS],
            count: 0,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        (0..MAX_GUARD_REGIONS).find(|&i| !self.regions[i].active)
    }

    fn find_by_id(&mut self, id: GuardRegionId) -> Option<&mut GuardRegion> {
        self.regions.get_mut(id.0 as usize)
    }

    /// Checks if `virt` falls into a registered guard page.
    fn lookup(&self, virt: u64) -> Option<usize> {
        for i in 0..MAX_GUARD_REGIONS {
            let r = &self.regions[i];
            if !r.active {
                continue;
            }
            let low_end = r.low_guard + PAGE_SIZE as u64;
            let high_end = r.high_guard + PAGE_SIZE as u64;
            if (virt >= r.low_guard && virt < low_end) || (virt >= r.high_guard && virt < high_end)
            {
                return Some(i);
            }
        }
        None
    }
}

static GUARD_TABLE: Mutex<GuardRegionTable> = Mutex::new(GuardRegionTable::new());

// ─────────────────────────────────────────────────────────────────────────────
// PTE Manipulation for Guard Pages
// ─────────────────────────────────────────────────────────────────────────────

/// Writes a guard PTE (non-present with tag) into `pte_ptr`.
///
/// # Safety
/// `pte_ptr` must point to a valid level 1 PTE entry (non-OOM).
pub unsafe fn write_guard_pte(pte_ptr: *mut u64) {
    pte_ptr.write_volatile(GUARD_PTE_VALUE);
    // Invalidate TLB for this entry.
    // Virtual address is unknown here; caller must invoke
    // invlpg on affected pages.
}

/// Returns `true` if `pte` is a guard entry (non-present + correct tag).
#[inline]
pub fn is_guard_pte(pte: u64) -> bool {
    // PRESENT = 0 AND bits 11:9 = 0b111.
    (pte & 1 == 0) && (pte & GUARD_PTE_MASK == GUARD_PTE_TAG)
}

/// Clears a guard PTE (resets to 0).
///
/// # Safety
/// `pte_ptr` must be valid.
pub unsafe fn clear_guard_pte(pte_ptr: *mut u64) {
    pte_ptr.write_volatile(0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Guard Region Registration
// ─────────────────────────────────────────────────────────────────────────────

/// Registers a guard region enclosing [`region_start`, `region_start + region_size`).
///
/// Guard pages are placed immediately BEFORE `region_start` (low guard)
/// and immediately AFTER `region_start + region_size` (high guard).
///
/// Caller is responsible for calling page table walker to write PTEs.
/// This function does not touch PTEs; it only maintains the directory.
///
/// Returns `Some(GuardRegionId)` or `None` if table full.
pub fn register_guard_region(
    region_start: u64,
    region_size: u64,
    kind: GuardRegionKind,
) -> Option<GuardRegionId> {
    let mut table = GUARD_TABLE.lock();
    let slot = table.alloc_slot()?;

    let low_guard = region_start.wrapping_sub(PAGE_SIZE as u64);
    let high_guard = region_start + region_size;

    table.regions[slot] = GuardRegion {
        low_guard,
        high_guard,
        region_start,
        region_size,
        kind,
        active: true,
    };
    table.count += 1;
    GUARD_STATS
        .regions_registered
        .fetch_add(1, Ordering::Relaxed);

    match kind {
        GuardRegionKind::KernelStack { .. } | GuardRegionKind::UserStack { .. } => GUARD_STATS
            .stack_guards_placed
            .fetch_add(1, Ordering::Relaxed),
        GuardRegionKind::KernelThreadStack { .. } => GUARD_STATS
            .stack_guards_placed
            .fetch_add(1, Ordering::Relaxed),
        GuardRegionKind::Vmalloc => GUARD_STATS
            .vmalloc_guards_placed
            .fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };

    Some(GuardRegionId(slot as u32))
}

/// Unregisters a guard region by its ID.
/// Caller must clear corresponding PTEs.
pub fn unregister_guard_region(id: GuardRegionId) -> bool {
    let mut table = GUARD_TABLE.lock();
    if let Some(region) = table.find_by_id(id) {
        if region.active {
            region.active = false;
            table.count -= 1;
            GUARD_STATS
                .regions_unregistered
                .fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Page Fault Detection
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a guard verification in fault handler.
#[derive(Debug, Clone, Copy)]
pub enum GuardFaultResult {
    /// Confirmed guard page -> kernel panic.
    GuardViolation {
        region_idx: usize,
        kind: GuardRegionKind,
    },
    /// Not a guard page -> normal fault.
    NotGuard,
}

/// Called by fault handler for any #PF.
/// If the faulting page is a guard page -> `GuardViolation`.
///
/// `fault_virt`: CR2 (faulting linear address).
/// `pte_value`: PTE value read from page table (0 if absent).
pub fn check_guard_fault(fault_virt: u64, pte_value: u64) -> GuardFaultResult {
    // Fast check on PTE tag.
    if pte_value != 0 && !is_guard_pte(pte_value) {
        return GuardFaultResult::NotGuard;
    }
    let table = GUARD_TABLE.lock();
    if let Some(idx) = table.lookup(fault_virt) {
        let kind = table.regions[idx].kind;
        drop(table);
        GUARD_STATS
            .violations_detected
            .fetch_add(1, Ordering::Relaxed);
        GuardFaultResult::GuardViolation {
            region_idx: idx,
            kind,
        }
    } else {
        GuardFaultResult::NotGuard
    }
}

/// Guard page violation handler — triggers a kernel panic.
pub fn guard_page_violation_handler(virt: u64, kind: GuardRegionKind) -> ! {
    GUARD_STATS
        .violations_detected
        .fetch_add(1, Ordering::Relaxed);
    match kind {
        GuardRegionKind::KernelStack { cpu_id } => panic!(
            "GUARD PAGE VIOLATION: kernel stack overflow cpu={} virt={:#x}",
            cpu_id, virt
        ),
        GuardRegionKind::KernelThreadStack { tid } => panic!(
            "GUARD PAGE VIOLATION: kernel thread stack overflow tid={} virt={:#x}",
            tid, virt
        ),
        GuardRegionKind::UserStack { tid } => panic!(
            "GUARD PAGE VIOLATION: user stack overflow tid={} virt={:#x}",
            tid, virt
        ),
        GuardRegionKind::Vmalloc => {
            panic!("GUARD PAGE VIOLATION: vmalloc overflow virt={:#x}", virt)
        }
        GuardRegionKind::Generic => panic!("GUARD PAGE VIOLATION: generic region virt={:#x}", virt),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CPU Stack Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical address of a kernel stack for `cpu_id`.
/// Convention: stacks at 0xFFFF_FF80_0000_0000 + cpu_id * 64 KiB.
const KERNEL_STACK_BASE: u64 = 0xFFFF_FF80_0000_0000;
const KERNEL_STACK_SIZE: u64 = 64 * 1024; // 64 KiB per CPU

/// Returns (stack_base, stack_size) for `cpu_id`.
pub fn cpu_stack_range(cpu_id: u32) -> (u64, u64) {
    let base = KERNEL_STACK_BASE + cpu_id as u64 * KERNEL_STACK_SIZE;
    (base, KERNEL_STACK_SIZE)
}

/// Registers guard pages for CPU `cpu_id`'s stack.
/// Returns `GuardRegionId` or `None` if table full.
pub fn register_cpu_stack_guards(cpu_id: u32) -> Option<GuardRegionId> {
    let (base, size) = cpu_stack_range(cpu_id);
    register_guard_region(base, size, GuardRegionKind::KernelStack { cpu_id })
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Init guard pages — registers guard pages for BSP (cpu_id=0).
pub fn init() {
    register_cpu_stack_guards(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_thread_stack_guard_region_catches_low_and_high_faults() {
        let region_start = 0xFFFF_9000_1000_0000;
        let region_size = (4 * PAGE_SIZE) as u64;
        let id = register_guard_region(
            region_start,
            region_size,
            GuardRegionKind::KernelThreadStack { tid: 77 },
        )
        .expect("guard slot");

        match check_guard_fault(region_start - PAGE_SIZE as u64, 0) {
            GuardFaultResult::GuardViolation {
                kind: GuardRegionKind::KernelThreadStack { tid },
                ..
            } => assert_eq!(tid, 77),
            other => panic!("expected low guard violation, got {other:?}"),
        }

        match check_guard_fault(region_start + region_size, GUARD_PTE_VALUE) {
            GuardFaultResult::GuardViolation {
                kind: GuardRegionKind::KernelThreadStack { tid },
                ..
            } => assert_eq!(tid, 77),
            other => panic!("expected high guard violation, got {other:?}"),
        }

        assert!(unregister_guard_region(id));
    }
}
