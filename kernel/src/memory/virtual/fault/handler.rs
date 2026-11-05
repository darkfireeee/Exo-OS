// kernel/src/memory/virtual/fault/handler.rs
//
// Page fault handler (#PF, vector 14).
// Dispatches to demand_paging, cow, or swap_in depending on the cause.
// Layer 0 — no external dependency except `spin`.

use super::{FaultCause, FaultContext, FaultResult};
use crate::memory::core::{AllocError, PageFlags, VirtAddr};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::PageTableEntry;
use crate::memory::virt::vma::{VmaBacking, VmaFlags};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// PAGE FAULT STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct FaultStats {
    pub total: AtomicU64,
    pub demand_paging: AtomicU64,
    pub cow_breaks: AtomicU64,
    pub swap_ins: AtomicU64,
    pub not_mapped: AtomicU64,
    pub protection: AtomicU64,
    pub kernel_faults: AtomicU64,
    pub oom_kills: AtomicU64,
}

impl Default for FaultStats {
    fn default() -> Self {
        Self::new()
    }
}

impl FaultStats {
    pub const fn new() -> Self {
        FaultStats {
            total: AtomicU64::new(0),
            demand_paging: AtomicU64::new(0),
            cow_breaks: AtomicU64::new(0),
            swap_ins: AtomicU64::new(0),
            not_mapped: AtomicU64::new(0),
            protection: AtomicU64::new(0),
            kernel_faults: AtomicU64::new(0),
            oom_kills: AtomicU64::new(0),
        }
    }
}

pub static FAULT_STATS: FaultStats = FaultStats::new();

fn present_permission_allowed(cause: FaultCause, flags: VmaFlags) -> bool {
    match cause {
        FaultCause::Read => flags.contains(VmaFlags::READ),
        FaultCause::Write => flags.contains(VmaFlags::WRITE),
        FaultCause::Execute => flags.contains(VmaFlags::EXEC),
    }
}

fn handle_present_permission_fault<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &crate::memory::virt::vma::VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr =
        VirtAddr::new(ctx.fault_addr.as_u64() & !(crate::memory::core::PAGE_SIZE as u64 - 1));
    let pte_raw = alloc.read_pte_raw(page_addr);
    let pte = PageTableEntry::from_raw(pte_raw);

    if ctx.cause == FaultCause::Write && (vma.flags.contains(VmaFlags::COW) || pte.is_cow()) {
        FAULT_STATS.cow_breaks.fetch_add(1, Ordering::Relaxed);
        return super::cow::handle_cow_fault(ctx, vma, alloc);
    }

    if !present_permission_allowed(ctx.cause, vma.flags) {
        FAULT_STATS.protection.fetch_add(1, Ordering::Relaxed);
        return FaultResult::Segfault {
            addr: ctx.fault_addr,
        };
    }

    if !pte.is_present() {
        FAULT_STATS.protection.fetch_add(1, Ordering::Relaxed);
        return FaultResult::Segfault {
            addr: ctx.fault_addr,
        };
    }

    let mut repaired_flags = vma.page_flags.set(PageFlags::PRESENT).set(PageFlags::USER);
    if ctx.cause == FaultCause::Write {
        repaired_flags = repaired_flags
            .set(PageFlags::WRITABLE)
            .clear(PageFlags::COW);
    }

    match alloc.remap_flags(page_addr, repaired_flags) {
        Ok(()) => {
            // SAFETY: page_addr is aligned and comes from CR2.
            unsafe {
                flush_single(page_addr);
            }
            FaultResult::Handled
        }
        Err(_) => {
            FAULT_STATS.protection.fetch_add(1, Ordering::Relaxed);
            FaultResult::Segfault {
                addr: ctx.fault_addr,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MAIN HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Handles a page fault.
///
/// `ctx` contains the faulting address (CR2), error code, and whether it was
/// in user or kernel mode.
///
/// Returns `FaultResult::Handled` if the fault was resolved and execution
/// can resume, or an error to trigger a signal/kill.
pub fn handle_page_fault<A: FaultAllocator>(ctx: &FaultContext, alloc: &A) -> FaultResult {
    FAULT_STATS.total.fetch_add(1, Ordering::Relaxed);
    if ctx.from_kernel {
        FAULT_STATS.kernel_faults.fetch_add(1, Ordering::Relaxed);
        // An unresolvable kernel fault = panic.
        return FaultResult::KernelFault {
            addr: ctx.fault_addr,
        };
    }

    // Find the VMA containing the faulting address.
    let vma = match ctx.find_vma(ctx.fault_addr) {
        Some(v) => v,
        None => {
            FAULT_STATS.not_mapped.fetch_add(1, Ordering::Relaxed);
            return FaultResult::Segfault {
                addr: ctx.fault_addr,
            };
        }
    };

    // Check VMA permissions.
    match ctx.cause {
        FaultCause::Write => {
            if !vma.flags.contains(VmaFlags::WRITE) && !vma.flags.contains(VmaFlags::COW) {
                FAULT_STATS.protection.fetch_add(1, Ordering::Relaxed);
                return FaultResult::Segfault {
                    addr: ctx.fault_addr,
                };
            }
        }
        FaultCause::Execute => {
            if !vma.flags.contains(VmaFlags::EXEC) {
                FAULT_STATS.protection.fetch_add(1, Ordering::Relaxed);
                return FaultResult::Segfault {
                    addr: ctx.fault_addr,
                };
            }
        }
        FaultCause::Read => {} // Always OK if the VMA is present
    }

    if ctx.present {
        return handle_present_permission_fault(ctx, vma, alloc);
    }

    // Dispatch according to cause.
    if ctx.cause == FaultCause::Write && vma.flags.contains(VmaFlags::COW) {
        // CoW break
        FAULT_STATS.cow_breaks.fetch_add(1, Ordering::Relaxed);
        return super::cow::handle_cow_fault(ctx, vma, alloc);
    }

    // Demand paging or swap-in
    if vma.flags.contains(VmaFlags::ANONYMOUS) || vma.backing == VmaBacking::File {
        match super::demand_paging::handle_demand_paging(ctx, vma, alloc) {
            FaultResult::Handled => {
                FAULT_STATS.demand_paging.fetch_add(1, Ordering::Relaxed);
                FaultResult::Handled
            }
            other => other,
        }
    } else {
        // Backup: swap-in
        FAULT_STATS.swap_ins.fetch_add(1, Ordering::Relaxed);
        super::swap_in::handle_swap_in(ctx, vma, alloc)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATION TRAIT FOR FAULT HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Allocation trait used by fault handlers.
pub trait FaultAllocator: Sync {
    fn alloc_zeroed(&self) -> Result<crate::memory::core::Frame, AllocError>;
    fn alloc_nonzeroed(&self) -> Result<crate::memory::core::Frame, AllocError>;
    fn free_frame(&self, f: crate::memory::core::Frame);
    fn map_page(
        &self,
        virt: VirtAddr,
        frame: crate::memory::core::Frame,
        flags: PageFlags,
    ) -> Result<(), AllocError>;
    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError>;
    fn translate(&self, virt: VirtAddr) -> Option<crate::memory::core::PhysAddr>;

    /// Reads the raw PTE value for the virtual address `virt`.
    ///
    /// Used by the swap-in handler to extract the swap entry stored
    /// in bits [63:1] of a PTE marked not-present (PRESENT=0).
    ///
    /// Returns `0` if the page has no known PTE (default implementation).
    fn read_pte_raw(&self, _virt: VirtAddr) -> u64 {
        0
    }

    /// Atomic compare-and-swap on the 4 KiB PTE of an already mapped page.
    ///
    /// Returns `Ok(())` if the exchange succeeded, or `Err(actual_raw)` if the
    /// observed PTE no longer matched `current`.
    fn compare_exchange_pte_raw(
        &self,
        virt: VirtAddr,
        _current: u64,
        _new: u64,
    ) -> Result<(), u64> {
        Err(self.read_pte_raw(virt))
    }
}
