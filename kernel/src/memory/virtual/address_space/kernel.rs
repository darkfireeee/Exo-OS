// kernel/src/memory/virtual/address_space/kernel.rs
//
// Kernel address space — singleton shared by all processes.
// Layer 0 — no external dependencies except `spin`.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::memory::core::{
    layout::VMALLOC_BASE, AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::x86_64::{phys_to_table_mut, phys_to_table_ref, read_cr3};
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker};

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL ADDRESS SPACE
// ─────────────────────────────────────────────────────────────────────────────

/// Global kernel address space.
///
/// Shared by all processes (upper half of PML4, indices 256..512).
/// Thread-safe via separate spinlocks for the vmalloc part and the
/// physmap part (the two most heavily contested concurrently).
pub struct KernelAddressSpace {
    inner: Mutex<KernelAsInner>,
    initialized: AtomicBool,
    pml4_phys: PhysAddr,
}

struct KernelAsInner {
    /// Bump pointer for vmalloc allocations.
    vmalloc_ptr: VirtAddr,
    /// Statistics.
    stats: KernelAsStats,
}

#[allow(dead_code)]
#[derive(Default)]
struct KernelAsStats {
    vmalloc_allocs: u64,
    vmalloc_frees: u64,
    map_calls: u64,
    unmap_calls: u64,
}

// SAFETY: KernelAddressSpace is thread-safe via its internal Mutex and
//         atomic accesses for unprotected fields.
unsafe impl Sync for KernelAddressSpace {}
unsafe impl Send for KernelAddressSpace {}

impl Default for KernelAddressSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelAddressSpace {
    pub const fn new() -> Self {
        KernelAddressSpace {
            inner: Mutex::new(KernelAsInner {
                vmalloc_ptr: VMALLOC_BASE,
                stats: KernelAsStats {
                    vmalloc_allocs: 0,
                    vmalloc_frees: 0,
                    map_calls: 0,
                    unmap_calls: 0,
                },
            }),
            initialized: AtomicBool::new(false),
            pml4_phys: PhysAddr::NULL,
        }
    }

    /// Initializes the kernel address space (boot-time, single-CPU).
    ///
    /// # Safety
    /// Must be called EXACTLY ONCE, before any other thread.
    pub unsafe fn init(&self, pml4_phys: PhysAddr) {
        debug_assert!(!self.initialized.load(Ordering::Relaxed));
        // SAFETY: Non-concurrent access on single-CPU — ptr::write avoids &mut T from &T.
        let pml4_ptr = core::ptr::addr_of!(self.pml4_phys) as *mut PhysAddr;
        pml4_ptr.write(pml4_phys);
        self.initialized.store(true, Ordering::Release);
    }

    /// Physical address of the kernel PML4.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Maps `frame` → `virt` with given flags in kernel space.
    ///
    /// # Safety
    /// `virt` must be in the upper half (>= 0xFFFF800000000000).
    pub unsafe fn map<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        debug_assert!(virt.is_kernel(), "KernelAS::map avec adresse user");
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        self.sync_active_kernel_half();
        flush_single(virt);
        self.inner.lock().stats.map_calls += 1;
        Ok(())
    }

    /// Unmaps `virt` in kernel space.
    ///
    /// # Safety
    /// `virt` must be in the upper half.
    pub unsafe fn unmap(&self, virt: VirtAddr) -> Option<Frame> {
        debug_assert!(virt.is_kernel());
        let mut walker = PageTableWalker::new(self.pml4_phys);
        let result = walker.unmap(virt);
        if result.is_some() {
            flush_single(virt);
            self.inner.lock().stats.unmap_calls += 1;
        }
        result
    }

    /// Allocates a vmalloc range and maps `n_pages` provided frames into it.
    ///
    /// Returns the starting virtual address.
    ///
    /// # Safety
    /// Frames must be valid and uncorrupted.
    pub unsafe fn vmalloc<A: FrameAllocatorForWalk>(
        &self,
        frames: &[Frame],
        flags: PageFlags,
        alloc: &A,
    ) -> Result<VirtAddr, AllocError> {
        let mut inner = self.inner.lock();
        let start = inner.vmalloc_ptr;
        let end = VirtAddr::new(start.as_u64() + (frames.len() * PAGE_SIZE) as u64);
        // Verify that the range is within vmalloc space
        if end.as_u64() > crate::memory::core::layout::MODULES_BASE.as_u64() {
            return Err(AllocError::OutOfMemory);
        }
        inner.vmalloc_ptr = end;
        inner.stats.vmalloc_allocs += 1;
        drop(inner);

        let mut walker = PageTableWalker::new(self.pml4_phys);
        for (i, &frame) in frames.iter().enumerate() {
            let v = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            walker.map(v, frame, flags, alloc)?;
        }
        self.sync_active_kernel_half();
        for i in 0..frames.len() {
            let v = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            flush_single(v);
        }
        Ok(start)
    }

    /// Reserves a vmalloc range without mapping it.
    ///
    /// Used by guarded kernel stacks: guard pages remain
    /// not present, then useful pages are mapped explicitly.
    pub fn reserve_vmalloc_pages(&self, n_pages: usize) -> Result<VirtAddr, AllocError> {
        if n_pages == 0 {
            return Err(AllocError::InvalidParams);
        }

        let mut inner = self.inner.lock();
        let start = inner.vmalloc_ptr;
        let bytes = n_pages
            .checked_mul(PAGE_SIZE)
            .ok_or(AllocError::InvalidParams)? as u64;
        let end = VirtAddr::new(start.as_u64().saturating_add(bytes));
        if end.as_u64() > crate::memory::core::layout::MODULES_BASE.as_u64() {
            return Err(AllocError::OutOfMemory);
        }

        inner.vmalloc_ptr = end;
        inner.stats.vmalloc_allocs += 1;
        Ok(start)
    }

    /// Copies current kernel PML4 entries into a target address space.
    ///
    /// User processes share upper half tables with the kernel. When a new
    /// vmalloc region is created after the construction of a user CR3, its
    /// PML4 entry must be visible before any switch to that CR3, because
    /// `context_switch_asm` loads CR3 before the kernel stack.
    ///
    /// # Safety
    /// `dst_pml4_phys` must point to a valid PML4 exclusive to the
    /// target process for its root level.
    pub unsafe fn sync_kernel_half_into(&self, dst_pml4_phys: PhysAddr) {
        if dst_pml4_phys.as_u64() == 0 || dst_pml4_phys == self.pml4_phys {
            return;
        }

        let src = phys_to_table_ref(self.pml4_phys);
        let dst = phys_to_table_mut(dst_pml4_phys);
        for i in 256..512 {
            dst[i] = src[i];
        }
    }

    /// Publishes new kernel entries into active CR3 if mapping from
    /// a syscall/IRQ running under userspace CR3.
    #[inline]
    fn sync_active_kernel_half(&self) {
        let active = read_cr3();
        if active.as_u64() != 0 && active != self.pml4_phys {
            unsafe {
                self.sync_kernel_half_into(active);
            }
        }
    }

    /// Translates a kernel virtual address into a physical address.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        let walker = PageTableWalker::new(self.pml4_phys);
        use crate::memory::virt::page_table::WalkResult;
        match walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let off = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + off))
            }
            _ => None,
        }
    }
}

/// Global kernel address space (singleton).
pub static KERNEL_AS: KernelAddressSpace = KernelAddressSpace::new();
