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
use crate::memory::virt::vma::{
    find_gap, mark_vma_cow, VmaBacking, VmaDescriptor, VmaFlags, VmaTree, MAX_VMAS_PER_PROCESS,
};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
    /// Serializes page-table unmap/remap vs remote-memory physical copies.
    pt_lock: Mutex<()>,
    /// Active remote address-space borrows (prevents `exec`/`wait` UAF).
    remote_refs: AtomicU32,
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
            pt_lock: Mutex::new(()),
            remote_refs: AtomicU32::new(0),
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

    /// Pins this address space against concurrent teardown (`exec`/`wait`).
    #[inline]
    pub fn pin_remote(&self) {
        self.remote_refs.fetch_add(1, Ordering::AcqRel);
    }

    /// Releases a remote address-space pin acquired via [`Self::pin_remote`].
    #[inline]
    pub fn unpin_remote(&self) {
        self.remote_refs.fetch_sub(1, Ordering::Release);
    }

    /// Waits until all temporary remote address-space borrowers have dropped.
    #[inline]
    pub fn wait_remote_unpinned(&self) {
        while self.remote_refs.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }
    }

    /// Serializes page-table unmap/remap against remote physical memory copies.
    #[inline]
    pub fn lock_pt(&self) -> spin::MutexGuard<'_, ()> {
        self.pt_lock.lock()
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

    /// Atomically removes the VMA starting at `start` only if its mapped
    /// range equals `[start, start + expected_len)`.
    /// Returns `Err(true)` if no VMA covers `start`, or `Err(false)` on length mismatch.
    pub fn remove_vma_exact(
        &self,
        start: VirtAddr,
        expected_len: u64,
    ) -> Result<*mut VmaDescriptor, bool> {
        let mut inner = self.inner.lock();
        let Some(vma) = inner.vma_tree.find(start) else {
            return Err(true);
        };
        if vma.start != start || vma.end.as_u64().saturating_sub(vma.start.as_u64()) != expected_len
        {
            return Err(false);
        }
        let result = inner.vma_tree.remove(start).ok_or(true)?;
        self.stats.vma_count.fetch_sub(1, Ordering::Relaxed);
        Ok(result)
    }

    /// Removes or splits the next VMA overlapping `[range_start, range_end)` under the VMA lock.
    /// Returns `Ok(Some((overlap_start, overlap_end, backing)))` for the unmapped chunk,
    /// `Ok(None)` when no more VMAs overlap the range, or `Err(true)` on OOM/limit.
    pub fn unmap_vma_range_next(
        &self,
        range_start: VirtAddr,
        range_end: VirtAddr,
    ) -> Result<Option<(VirtAddr, VirtAddr, VmaBacking)>, bool> {
        let mut inner = self.inner.lock();
        let Some(vma_ref) = inner.vma_tree.find_first_overlapping(range_start, range_end) else {
            return Ok(None);
        };

        let v_start = vma_ref.start;
        let v_end = vma_ref.end;
        let v_backing = vma_ref.backing;
        let v_flags = vma_ref.flags;
        let v_page_flags = vma_ref.page_flags;
        let v_inode = vma_ref.inode_id;
        let v_off = vma_ref.file_offset;

        let overlap_start = VirtAddr::new(v_start.as_u64().max(range_start.as_u64()));
        let overlap_end = VirtAddr::new(v_end.as_u64().min(range_end.as_u64()));
        let has_left = v_start.as_u64() < overlap_start.as_u64();
        let has_right = overlap_end.as_u64() < v_end.as_u64();

        match (has_left, has_right) {
            (false, false) => {
                let ptr = inner.vma_tree.remove(v_start).ok_or(true)?;
                self.stats.vma_count.fetch_sub(1, Ordering::Relaxed);
                // SAFETY: `remove` transfers descriptor ownership out of the tree.
                let _ = unsafe { Box::from_raw(ptr) };
            }
            (true, false) => {
                let vma_mut = inner.vma_tree.find_mut(v_start).ok_or(true)?;
                vma_mut.end = overlap_start;
            }
            (false, true) => {
                let ptr = inner.vma_tree.remove(v_start).ok_or(true)?;
                // SAFETY: exclusive tree lock held and `ptr` is uniquely owned here.
                unsafe {
                    (*ptr).start = overlap_end;
                    (*ptr).file_offset = v_off + (overlap_end.as_u64() - v_start.as_u64());
                    if !inner.vma_tree.insert(ptr) {
                        let _ = Box::from_raw(ptr);
                        return Err(true);
                    }
                }
            }
            (true, true) => {
                if inner.vma_tree.len() >= MAX_VMAS_PER_PROCESS {
                    return Err(true);
                }
                let mut right_desc =
                    VmaDescriptor::new(overlap_end, v_end, v_flags, v_page_flags, v_backing);
                right_desc.inode_id = v_inode;
                right_desc.file_offset = v_off + (overlap_end.as_u64() - v_start.as_u64());
                let right_ptr = Box::into_raw(Box::new(right_desc));

                let Some(vma_mut) = inner.vma_tree.find_mut(v_start) else {
                    let _ = unsafe { Box::from_raw(right_ptr) };
                    return Err(true);
                };
                vma_mut.end = overlap_start;
                // SAFETY: `right_ptr` does not overlap the shrunken left VMA or any other VMA.
                if !unsafe { inner.vma_tree.insert(right_ptr) } {
                    let _ = unsafe { Box::from_raw(right_ptr) };
                    return Err(true);
                }
                self.stats.vma_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        Ok(Some((overlap_start, overlap_end, v_backing)))
    }

    /// Returns `true` iff `[start, end)` is completely covered by VMAs with no holes.
    pub fn verify_vma_range_covered(&self, start: VirtAddr, end: VirtAddr) -> bool {
        let inner = self.inner.lock();
        let mut cursor = start.as_u64();
        let end_raw = end.as_u64();
        while cursor < end_raw {
            let Some(vma) = inner.vma_tree.find(VirtAddr::new(cursor)) else {
                return false;
            };
            if vma.end.as_u64() <= cursor {
                return false;
            }
            cursor = vma.end.as_u64().min(end_raw);
        }
        true
    }

    /// Splits (if partial) and updates VMA permissions for the next chunk starting at `cursor`
    /// within `[cursor, range_end)` while holding `self.inner`.
    /// Returns `Ok(Some((sub_start, sub_end, effective_page_flags, is_shared)))`,
    /// `Ok(None)` if `cursor >= range_end`, `Err(false)` if not mapped, or `Err(true)` on limit.
    pub fn protect_vma_range_next(
        &self,
        cursor: VirtAddr,
        range_end: VirtAddr,
        base_page_flags: PageFlags,
        new_perm_vma: VmaFlags,
    ) -> Result<Option<(VirtAddr, VirtAddr, PageFlags, bool)>, bool> {
        if cursor.as_u64() >= range_end.as_u64() {
            return Ok(None);
        }
        let mut inner = self.inner.lock();
        let Some(vma_ref) = inner.vma_tree.find(cursor) else {
            return Err(false);
        };

        let v_start = vma_ref.start;
        let v_end = vma_ref.end;
        let v_flags = vma_ref.flags;
        let v_page_flags = vma_ref.page_flags;
        let v_backing = vma_ref.backing;
        let v_inode = vma_ref.inode_id;
        let v_off = vma_ref.file_offset;

        let sub_start = cursor;
        let sub_end = VirtAddr::new(v_end.as_u64().min(range_end.as_u64()));
        let is_shared = v_backing != VmaBacking::Anonymous || v_flags.contains(VmaFlags::SHARED);
        let preserve_pinned = is_shared || v_page_flags.contains(PageFlags::PINNED);
        let effective_page_flags = if preserve_pinned {
            base_page_flags.set(PageFlags::PINNED)
        } else {
            base_page_flags
        };
        let perm_mask = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::EXEC;
        let effective_vma_flags =
            VmaFlags::from_bits(v_flags.bits() & !perm_mask.bits()) | new_perm_vma;

        let has_before = sub_start.as_u64() > v_start.as_u64();
        let has_after = sub_end.as_u64() < v_end.as_u64();
        let added = (has_before as usize) + (has_after as usize);
        if inner.vma_tree.len() + added > MAX_VMAS_PER_PROCESS {
            return Err(true);
        }

        let after_ptr = if has_after {
            let mut after_desc =
                VmaDescriptor::new(sub_end, v_end, v_flags, v_page_flags, v_backing);
            after_desc.inode_id = v_inode;
            after_desc.file_offset = v_off + (sub_end.as_u64() - v_start.as_u64());
            Some(Box::into_raw(Box::new(after_desc)))
        } else {
            None
        };

        if has_before {
            let mut middle_desc = VmaDescriptor::new(
                sub_start,
                sub_end,
                effective_vma_flags,
                effective_page_flags,
                v_backing,
            );
            middle_desc.inode_id = v_inode;
            middle_desc.file_offset = v_off + (sub_start.as_u64() - v_start.as_u64());
            let middle_ptr = Box::into_raw(Box::new(middle_desc));

            let Some(vma_mut) = inner.vma_tree.find_mut(cursor) else {
                unsafe {
                    drop(Box::from_raw(middle_ptr));
                    if let Some(ap) = after_ptr {
                        drop(Box::from_raw(ap));
                    }
                }
                return Err(false);
            };
            vma_mut.end = sub_start;
            // SAFETY: `middle_ptr` lies strictly after `sub_start` and before `after_ptr`.
            if !unsafe { inner.vma_tree.insert(middle_ptr) } {
                unsafe {
                    drop(Box::from_raw(middle_ptr));
                    if let Some(ap) = after_ptr {
                        drop(Box::from_raw(ap));
                    }
                }
                return Err(true);
            }
        } else {
            let Some(vma_mut) = inner.vma_tree.find_mut(cursor) else {
                if let Some(ap) = after_ptr {
                    unsafe { drop(Box::from_raw(ap)) };
                }
                return Err(false);
            };
            vma_mut.end = sub_end;
            vma_mut.flags = effective_vma_flags;
            vma_mut.page_flags = effective_page_flags;
        }

        if let Some(ap) = after_ptr {
            // SAFETY: `ap` lies strictly in `[sub_end, v_end)` after `middle`.
            if !unsafe { inner.vma_tree.insert(ap) } {
                unsafe { drop(Box::from_raw(ap)) };
                return Err(true);
            }
        }

        if added > 0 {
            self.stats.vma_count.fetch_add(added as u64, Ordering::Relaxed);
        }

        Ok(Some((sub_start, sub_end, effective_page_flags, is_shared)))
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
