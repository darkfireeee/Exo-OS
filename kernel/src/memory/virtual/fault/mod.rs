// kernel/src/memory/virtual/fault/mod.rs
//
// Module fault — gestionnaire de page faults.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod cow;
pub mod demand_paging;
pub mod handler;
pub mod swap_in;

use crate::memory::core::VirtAddr;
use crate::memory::virt::vma::VmaDescriptor;

// ─────────────────────────────────────────────────────────────────────────────
// TYPES PARTAGÉS
// ─────────────────────────────────────────────────────────────────────────────

/// Cause d'un page fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultCause {
    Read,
    Write,
    Execute,
}

/// Contexte d'un page fault (construit depuis le handler d'interruption).
pub struct FaultContext {
    /// Adresse virtuelle fautive (contenu de CR2).
    pub fault_addr: VirtAddr,
    /// Cause du fault.
    pub cause: FaultCause,
    /// Fault depuis le mode kernel (Ring 0) ?
    pub from_kernel: bool,
    /// Pointeur vers la VMA de l'espace utilisateur (lookup déjà fait, facultatif).
    vma_ptr: *const VmaDescriptor,
}

impl FaultContext {
    pub fn new(fault_addr: VirtAddr, cause: FaultCause, from_kernel: bool) -> Self {
        FaultContext {
            fault_addr,
            cause,
            from_kernel,
            vma_ptr: core::ptr::null(),
        }
    }

    pub fn with_vma(mut self, vma: *const VmaDescriptor) -> Self {
        self.vma_ptr = vma;
        self
    }

    /// Retourne le VmaDescriptor si disponible dans le contexte.
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        if self.vma_ptr.is_null() {
            return None;
        }
        // SAFETY: vma_ptr est valide si fourni par l'address space.
        let vma = unsafe { &*self.vma_ptr };
        if vma.contains(addr) {
            Some(vma)
        } else {
            None
        }
    }
}

/// Résultat du traitement d'un page fault.
#[derive(Debug)]
pub enum FaultResult {
    /// Fault résolu, reprise de l'exécution.
    Handled,
    /// Segmentation fault — envoyer SIGSEGV.
    Segfault { addr: VirtAddr },
    /// Out of memory — tuer le processus.
    Oom { addr: VirtAddr },
    /// Fault kernel non récupérable — panic.
    KernelFault { addr: VirtAddr },
}

impl FaultResult {
    /// Retourne `true` si le fault a été résolu avec succès.
    #[inline]
    pub fn is_handled(&self) -> bool {
        matches!(self, FaultResult::Handled)
    }
}

pub use handler::{handle_page_fault, FaultAllocator, FAULT_STATS};
pub use swap_in::SwapInProvider;
