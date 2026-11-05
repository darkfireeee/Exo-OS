// kernel/src/memory/virtual/fault/mod.rs
//
// fault module — page fault handler.
// Layer 0 — no external dependencies except `spin`.

pub mod cow;
pub mod demand_paging;
pub mod handler;
pub mod swap_in;

use crate::memory::core::VirtAddr;
use crate::memory::virt::vma::VmaDescriptor;

// ─────────────────────────────────────────────────────────────────────────────
// SHARED TYPES
// ─────────────────────────────────────────────────────────────────────────────

/// Cause of a page fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultCause {
    Read,
    Write,
    Execute,
}

/// Context of a page fault (built from the interrupt handler).
pub struct FaultContext {
    /// Faulting virtual address (content of CR2).
    pub fault_addr: VirtAddr,
    /// Cause of fault.
    pub cause: FaultCause,
    /// P bit of x86 error-code: true means protection fault on an
    /// already present translation, not demand paging.
    pub present: bool,
    /// Fault from kernel mode (Ring 0)?
    pub from_kernel: bool,
    /// Pointer to user space VMA (lookup already performed, optional).
    vma_ptr: *const VmaDescriptor,
}

impl FaultContext {
    pub fn new(fault_addr: VirtAddr, cause: FaultCause, from_kernel: bool) -> Self {
        FaultContext {
            fault_addr,
            cause,
            present: false,
            from_kernel,
            vma_ptr: core::ptr::null(),
        }
    }

    pub fn with_present(mut self, present: bool) -> Self {
        self.present = present;
        self
    }

    pub fn with_vma(mut self, vma: *const VmaDescriptor) -> Self {
        self.vma_ptr = vma;
        self
    }

    /// Returns the VmaDescriptor if available in context.
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        if self.vma_ptr.is_null() {
            return None;
        }
        // SAFETY: vma_ptr is valid if provided by the address space.
        let vma = unsafe { &*self.vma_ptr };
        if vma.contains(addr) {
            Some(vma)
        } else {
            None
        }
    }
}

/// Result of handling a page fault.
#[derive(Debug)]
pub enum FaultResult {
    /// Fault resolved, resume execution.
    Handled,
    /// Segmentation fault — send SIGSEGV.
    Segfault { addr: VirtAddr },
    /// Out of memory — kill process.
    Oom { addr: VirtAddr },
    /// Unrecoverable kernel fault — panic.
    KernelFault { addr: VirtAddr },
}

impl FaultResult {
    /// Returns `true` if fault was handled successfully.
    #[inline]
    pub fn is_handled(&self) -> bool {
        matches!(self, FaultResult::Handled)
    }
}

pub use handler::{handle_page_fault, FaultAllocator, FAULT_STATS};
pub use swap_in::SwapInProvider;
