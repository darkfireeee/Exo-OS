// kernel/src/memory/virtual/address_space/user.rs
//
// User address space — one per process.
// Layer 0 — no external dependencies except `spin`.

use crate::memory::core::{
    layout::{USER_END, USER_STACK_TOP},
    AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker, WalkResult};
use crate::memory::virt::vma::{find_gap, mark_vma_cow, VmaDescriptor, VmaFlags, VmaTree};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// USER ADDRESS SPACE
// ─────────────────────────────────────────────────────────────────────────────

/// Default layout constants for user space.
pub const USER_MMAP_BASE: u64 = 0x0000_7F00_0000_0000;
pub const USER_STACK_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

/// Statistics of user address space.
pub struct UserAsStats {
    pub page_faults: AtomicU64,
    pub cow_breaks: AtomicU64,
    pub mmap_calls: AtomicU64,
    pub munmap_calls: AtomicU64,
    pub vma_count: AtomicU64,
}

impl Default for UserAsStats {
    fn default() -> Self {
        Self::new()
    }
}

impl UserAsStats {
    pub const fn new() -> Self {
        UserAsStats {
            page_faults: AtomicU64::new(0),
            cow_breaks: AtomicU64::new(0),
            mmap_calls: AtomicU64::new(0),
            munmap_calls: AtomicU64::new(0),
            vma_count: AtomicU64::new(0),
        }
    }
}

/// Address space of a user process.
pub struct UserAddressSpace {
    inner: Mutex<UserAsInner>,
    pub stats: UserAsStats,
    pml4_phys: PhysAddr,
    /// Associated process ID (for targeted TLB shootdown).
    pub pid: u64,
    /// ELF base of user heap. 0 indicates that no ELF has published its
    /// initial break yet and the historical brk() fallback must be used.
    pub heap_start: AtomicU64,
    /// Current absolute break of user heap.
    pub heap_end: AtomicU64,
}

#[allow(dead_code)]
struct UserAsInner {
    vma_tree: VmaTree,
    mmap_hint: VirtAddr, // Hint for mmap (bump)
    stack_bottom: VirtAddr,
}

// SAFETY: UserAddressSpace is thread-safe via its internal Mutex.
unsafe impl Sync for UserAddressSpace {}
unsafe impl Send for UserAddressSpace {}

impl UserAddressSpace {
    /// Creates a new empty user address space.
    ///
    /// `pml4_phys` must have already been constructed (cloned kernel entries).
    pub fn new(pml4_phys: PhysAddr, pid: u64) -> Self {
        let stack_bottom = VirtAddr::new(USER_STACK_TOP.as_u64() - USER_STACK_SIZE);
        UserAddressSpace {
            inner: Mutex::new(UserAsInner {
                vma_tree: VmaTree::new(),
                mmap_hint: VirtAddr::new(USER_MMAP_BASE),
                stack_bottom,
            }),
            stats: UserAsStats::new(),
            pml4_phys,
            pid,
            heap_start: AtomicU64::new(0),
            heap_end: AtomicU64::new(0),
        }
    }

    /// Physical address of this space's PML4.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Initializes brk bounds of this space after ELF loading.
    #[inline]
    pub fn init_heap_bounds(&self, brk_start: u64) {
        self.heap_start.store(brk_start, Ordering::Release);
        self.heap_end.store(brk_start, Ordering::Release);
    }

    /// Returns the first address not covered by HEAP VMAs in
    /// `[start, end)`. `None` indicates a non-heap VMA occupies the range.
    pub fn heap_covered_end_from(&self, start: VirtAddr, end: VirtAddr) -> Option<VirtAddr> {
        let inner = self.inner.lock();
        let mut cursor = start.as_u64();
        let end_raw = end.as_u64();

        while cursor < end_raw {
            let addr = VirtAddr::new(cursor);
            let Some(vma) = inner.vma_tree.find(addr) else {
                return Some(addr);
            };
            if !vma.flags.contains(VmaFlags::HEAP) {
                return None;
            }
            let next = vma.end.as_u64();
            if next <= cursor {
                return None;
            }
            cursor = next.min(end_raw);
        }

        Some(end)
    }

    /// Maps `virt` → `frame` directly (without VMA — for the ELF loader).
    ///
    /// # Safety
    /// `virt` must be in user space (< USER_END).
    pub unsafe fn map_page<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        debug_assert!(
            virt.as_u64() < USER_END.as_u64(),
            "map_page : adresse hors user"
        );
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        flush_single(virt);
        Ok(())
    }

    /// Maps `virt` -> `frame` without invalidating the TLB.
    ///
    /// # Safety
    /// Use only for a freshly reserved userspace range, before
    /// publishing the VMA and before any user access to this range.
    /// Paths that replace or unmap a page must continue using
    /// `map_page()` or perform their own invalidation.
    pub unsafe fn map_page_unflushed<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        // PATCH-MEM-01: hard assert (not debug_assert) — in release, corruption
        // would be silent if virt >= USER_END (overwriting kernel space).
        assert!(
            virt.as_u64() < USER_END.as_u64(),
            "map_page_unflushed: adresse hors espace utilisateur: {:#x} >= USER_END {:#x}",
            virt.as_u64(),
            USER_END.as_u64()
        );
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        Ok(())
    }

    /// Unmaps `virt` (without VMA).
    ///
    /// # Safety
    /// `virt` must be in the user address space of this table.
    pub unsafe fn unmap_page(&self, virt: VirtAddr) -> Option<Frame> {
        let mut walker = PageTableWalker::new(self.pml4_phys);
        let result = walker.unmap(virt);
        if result.is_some() {
            flush_single(virt);
        }
        result
    }

    /// Translates a user virtual address into physical.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        let walker = PageTableWalker::new(self.pml4_phys);
        match walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let off = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + off))
            }
            _ => None,
        }
    }

    /// Finds the VMA containing `addr`.
    pub fn find_vma(&self, addr: VirtAddr) -> Option<*const VmaDescriptor> {
        let inner = self.inner.lock();
        inner.vma_tree.find(addr).map(|v| v as *const _)
    }

    /// Adds flags to the VMA containing `addr` (PROC-VMA / V-17).
    pub fn set_vma_flags(&self, addr: VirtAddr, extra: VmaFlags) -> bool {
        let mut inner = self.inner.lock();
        if let Some(vma) = inner.vma_tree.find_mut(addr) {
            vma.flags |= extra;
            true
        } else {
            false
        }
    }

    /// Marks all private writable VMAs as CoW after fork().
    pub fn mark_all_writable_vmas_cow(&self) {
        let mut inner = self.inner.lock();
        inner.vma_tree.for_each_mut(mark_vma_cow);
    }

    /// Clones internal metadata from parent to fork() child.
    pub fn clone_inner_for_fork(&self, child: &UserAddressSpace) -> bool {
        let src = self.inner.lock();
        let Some(cloned_tree) = src.vma_tree.clone_for_fork() else {
            return false;
        };
        let cloned_count = cloned_tree.len() as u64;

        let mut dst = child.inner.lock();
        dst.vma_tree = cloned_tree;
        dst.mmap_hint = src.mmap_hint;
        dst.stack_bottom = src.stack_bottom;
        child.stats.vma_count.store(cloned_count, Ordering::Relaxed);
        true
    }

    /// Registers a new VMA in the address space.
    ///
    /// # Safety
    /// The descriptor must have been allocated by the slab and the caller
    /// yields ownership to the VmaTree.
    pub unsafe fn insert_vma(&self, vma: *mut VmaDescriptor) -> bool {
        let mut inner = self.inner.lock();
        let result = inner.vma_tree.insert(vma);
        if result {
            self.stats.vma_count.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Removes the VMA starting at `start`.
    pub fn remove_vma(&self, start: VirtAddr) -> Option<*mut VmaDescriptor> {
        let mut inner = self.inner.lock();
        let result = inner.vma_tree.remove(start);
        if result.is_some() {
            self.stats.vma_count.fetch_sub(1, Ordering::Relaxed);
        }
        result
    }

    /// Searches for a free gap of `size` bytes in user space.
    pub fn find_free_gap(&self, size: usize, hint: Option<VirtAddr>) -> Option<VirtAddr> {
        let inner = self.inner.lock();
        let hint_addr = hint.unwrap_or(inner.mmap_hint);
        find_gap(
            &inner.vma_tree,
            size,
            hint_addr,
            VirtAddr::new(PAGE_SIZE as u64), // min = 4 KiB (avoid NULL)
            USER_END,
        )
    }

    /// Increments page faults counter.
    pub fn record_fault(&self) {
        self.stats.page_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments CoW breaks counter.
    pub fn record_cow_break(&self) {
        self.stats.cow_breaks.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of VMAs in this space.
    pub fn vma_count(&self) -> usize {
        self.inner.lock().vma_tree.len()
    }
}
