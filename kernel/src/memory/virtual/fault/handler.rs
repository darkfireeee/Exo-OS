// kernel/src/memory/virtual/fault/handler.rs
//
// Gestionnaire de page fault (#PF, vecteur 14).
// Dispatche vers demand_paging, cow, ou swap_in selon la cause.
// Couche 0 — aucune dépendance externe sauf `spin`.

use super::{FaultCause, FaultContext, FaultResult};
use crate::memory::core::{AllocError, PageFlags, VirtAddr};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::PageTableEntry;
use crate::memory::virt::vma::{VmaBacking, VmaFlags};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES DE PAGE FAULT
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
            // SAFETY: page_addr est alignee et provient de CR2.
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
// HANDLER PRINCIPAL
// ─────────────────────────────────────────────────────────────────────────────

/// Traite un page fault.
///
/// `ctx` contient l'adresse fautive (CR2), l'error code et si l'on était
/// en mode user ou kernel.
///
/// Retourne `FaultResult::Handled` si le fault a été résolu et l'exécution
/// peut reprendre, ou une erreur pour déclencher un signal/kill.
pub fn handle_page_fault<A: FaultAllocator>(ctx: &FaultContext, alloc: &A) -> FaultResult {
    FAULT_STATS.total.fetch_add(1, Ordering::Relaxed);
    if ctx.from_kernel {
        FAULT_STATS.kernel_faults.fetch_add(1, Ordering::Relaxed);
        // Un fault kernel non résolvable = panic.
        return FaultResult::KernelFault {
            addr: ctx.fault_addr,
        };
    }

    // Trouver la VMA qui contient l'adresse fautive.
    let vma = match ctx.find_vma(ctx.fault_addr) {
        Some(v) => v,
        None => {
            FAULT_STATS.not_mapped.fetch_add(1, Ordering::Relaxed);
            return FaultResult::Segfault {
                addr: ctx.fault_addr,
            };
        }
    };

    // Vérifier les permissions de la VMA.
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
        FaultCause::Read => {} // Toujours OK si la VMA est présente
    }

    if ctx.present {
        return handle_present_permission_fault(ctx, vma, alloc);
    }

    // Dispatcher selon la cause.
    if ctx.cause == FaultCause::Write && vma.flags.contains(VmaFlags::COW) {
        // CoW break
        FAULT_STATS.cow_breaks.fetch_add(1, Ordering::Relaxed);
        return super::cow::handle_cow_fault(ctx, vma, alloc);
    }

    // Demand paging ou swap-in
    if vma.flags.contains(VmaFlags::ANONYMOUS) || vma.backing == VmaBacking::File {
        match super::demand_paging::handle_demand_paging(ctx, vma, alloc) {
            FaultResult::Handled => {
                FAULT_STATS.demand_paging.fetch_add(1, Ordering::Relaxed);
                FaultResult::Handled
            }
            other => other,
        }
    } else {
        // Backup : swap-in
        FAULT_STATS.swap_ins.fetch_add(1, Ordering::Relaxed);
        super::swap_in::handle_swap_in(ctx, vma, alloc)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT D'ALLOCATION POUR LE FAULT HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Trait d'allocation utilisé par les handlers de fault.
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

    /// Lit la valeur brute de la PTE pour l'adresse virtuelle `virt`.
    ///
    /// Utilisé par le swap-in handler pour extraire l'entrée de swap stockée
    /// dans les bits [63:1] d'une PTE marquée non-présente (PRESENT=0).
    ///
    /// Retourne `0` si la page n'a pas de PTE connue (implémentation par défaut).
    fn read_pte_raw(&self, _virt: VirtAddr) -> u64 {
        0
    }

    /// Compare/exchange atomique sur la PTE 4 KiB d'une page déjà mappée.
    ///
    /// Retourne `Ok(())` si l'échange a réussi, ou `Err(actual_raw)` si la PTE
    /// observée ne correspondait plus à `current`.
    fn compare_exchange_pte_raw(
        &self,
        virt: VirtAddr,
        _current: u64,
        _new: u64,
    ) -> Result<(), u64> {
        Err(self.read_pte_raw(virt))
    }
}
