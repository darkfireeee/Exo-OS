// kernel/src/memory/virtual/page_table/walker.rs
//
// Page table walker — recursive traversal PML4→PDPT→PD→PT.
// Handles walking with on-demand allocation or read-only mode.
// Layer 0 — no external dependency except `spin`.

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::virt::page_table::x86_64::{
    phys_to_table_mut, phys_to_table_ref, PageTable, PageTableEntry, PageTableLevel,
};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// WALK RESULT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of walking a page table down to a leaf.
#[derive(Debug)]
pub enum WalkResult {
    /// Leaf found (PT entry + index).
    Leaf {
        entry: PageTableEntry,
        level: PageTableLevel,
    },
    /// Page not present at this level.
    NotMapped,
    /// Huge page (2 MiB or 1 GiB) detected before reaching level 1.
    HugePage {
        entry: PageTableEntry,
        level: PageTableLevel,
    },
    /// Error during intermediate level allocation.
    AllocError(AllocError),
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATION TRAIT FOR WALKER
// ─────────────────────────────────────────────────────────────────────────────

/// Allows the walker to allocate intermediate tables without depending
/// directly on the buddy allocator (dependency inversion).
pub trait FrameAllocatorForWalk {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError>;
    fn free_frame(&self, frame: Frame);
}

// ─────────────────────────────────────────────────────────────────────────────
// WALKER
// ─────────────────────────────────────────────────────────────────────────────

/// x86_64 page table walker.
/// Traverses all 4 levels to find or create a mapping.
pub struct PageTableWalker {
    pml4_phys: PhysAddr,
}

impl PageTableWalker {
    /// Creates a walker for the PML4 at `pml4_phys`.
    pub fn new(pml4_phys: PhysAddr) -> Self {
        PageTableWalker { pml4_phys }
    }

    /// Read-only walk down to the leaf for `virt`.
    /// Does not create any intermediate level.
    pub fn walk_read(&self, virt: VirtAddr) -> WalkResult {
        // SAFETY: pml4_phys is a valid PML4 initialized before any walk.
        let pml4 = unsafe { phys_to_table_ref(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return WalkResult::NotMapped;
        }

        // SAFETY: l4_entry.phys_addr() points to a valid PDPT.
        let pdpt = unsafe { phys_to_table_ref(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return WalkResult::NotMapped;
        }
        if l3_entry.is_huge() {
            return WalkResult::HugePage {
                entry: l3_entry,
                level: PageTableLevel::L3,
            };
        }

        // SAFETY: l3_entry.phys_addr() points to a valid PD.
        let pd = unsafe { phys_to_table_ref(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return WalkResult::NotMapped;
        }
        if l2_entry.is_huge() {
            return WalkResult::HugePage {
                entry: l2_entry,
                level: PageTableLevel::L2,
            };
        }

        // SAFETY: l2_entry.phys_addr() points to a valid PT.
        let pt = unsafe { phys_to_table_ref(l2_entry.phys_addr()) };
        let l1_entry = pt[virt.p1_index()];
        if !l1_entry.is_present() {
            return WalkResult::NotMapped;
        }
        WalkResult::Leaf {
            entry: l1_entry,
            level: PageTableLevel::L1,
        }
    }

    /// Maps `virt` → `frame` with the given flags.
    ///
    /// Creates missing intermediate levels using `alloc`.
    /// Returns `AllocError` if an intermediate page cannot be allocated.
    pub fn map<A: FrameAllocatorForWalk>(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        // SAFETY: pml4_phys is a valid PML4 (initialized beforehand).
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, virt.p4_index(), alloc, flags)?;
        // SAFETY: l3_phys points to a valid or freshly allocated PDPT.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, virt.p3_index(), alloc, flags)?;
        // SAFETY: l2_phys points to a valid or freshly allocated PD.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let l1_phys = self.ensure_table(pd, virt.p2_index(), alloc, flags)?;
        // SAFETY: l1_phys points to a valid or freshly allocated PT.
        let pt = unsafe { phys_to_table_mut(l1_phys) };

        let idx = virt.p1_index();
        let entry = &mut pt[idx];
        let new_entry = PageTableEntry::from_page_flags(frame, flags);
        *entry = new_entry;
        Ok(())
    }

    /// Maps a 2 MiB huge page at the PD level.
    ///
    /// Used for kernel physmap: intermediate tables are created
    /// normally, but the leaf is a PD entry with the PS bit.
    pub fn map_huge_2m<A: FrameAllocatorForWalk>(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        const HUGE_2M: u64 = 2 * 1024 * 1024;
        if (virt.as_u64() & (HUGE_2M - 1)) != 0 || !phys.is_aligned(HUGE_2M) {
            return Err(AllocError::InvalidParams);
        }

        let table_flags = flags.clear(PageFlags::HUGE_PAGE);
        let huge_flags = flags.set(PageFlags::HUGE_PAGE);
        // SAFETY: pml4_phys is a valid PML4.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, virt.p4_index(), alloc, table_flags)?;
        // SAFETY: l3_phys points to a valid or freshly allocated PDPT.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, virt.p3_index(), alloc, table_flags)?;
        // SAFETY: l2_phys points to a valid or freshly allocated PD.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let entry = &mut pd[virt.p2_index()];
        if entry.is_present() && !entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        *entry = PageTableEntry::from_page_flags(Frame::containing(phys), huge_flags);
        Ok(())
    }

    /// Unmaps `virt`.
    ///
    /// Returns the previously mapped frame, or `None` if not mapped.
    /// Also returns `None` if a huge page is encountered along the way
    /// (use a huge-page specific unmap in that case).
    pub fn unmap(&mut self, virt: VirtAddr) -> Option<Frame> {
        // SAFETY: pml4_phys is a valid PML4.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return None;
        }

        // SAFETY: l4_entry.phys_addr() is a valid PDPT.
        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return None;
        }
        // 1 GiB huge page: cannot be decomposed into 4 KiB here
        if l3_entry.is_huge() {
            return None;
        }

        // SAFETY: l3_entry.phys_addr() is a valid PD.
        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return None;
        }
        // 2 MiB huge page: same
        if l2_entry.is_huge() {
            return None;
        }

        // SAFETY: l2_entry.phys_addr() is a valid PT.
        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        let l1_idx = virt.p1_index();
        let old = pt[l1_idx];
        if !old.is_present() {
            return None;
        }
        pt[l1_idx].clear();
        old.frame()
    }

    /// Modifies the flags of an existing mapping.
    pub fn remap_flags(&mut self, virt: VirtAddr, new_flags: PageFlags) -> Result<(), AllocError> {
        // SAFETY: pml4_phys is a valid PML4.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        let l4_entry = &mut pml4[virt.p4_index()];
        Self::upgrade_table_entry_permissions(l4_entry, new_flags);

        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // 1 GiB huge page: this function only manages 4 KiB mappings
        if l3_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        let l3_entry = &mut pdpt[virt.p3_index()];
        Self::upgrade_table_entry_permissions(l3_entry, new_flags);

        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // 2 MiB huge page: same
        if l2_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        let l2_entry = &mut pd[virt.p2_index()];
        Self::upgrade_table_entry_permissions(l2_entry, new_flags);

        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        let entry = &mut pt[virt.p1_index()];
        if !entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        let frame = entry.frame().ok_or(AllocError::InvalidParams)?;
        *entry = PageTableEntry::from_page_flags(frame, new_flags);
        Ok(())
    }

    /// Moves a 4 KiB leaf PTE from `src` to `dst` without copying the frame.
    ///
    /// Destination intermediate levels are allocated before modifying
    /// the source, so an allocation failure does not lose the original mapping.
    /// Returns `Ok(false)` if `src` is not resident (demand paging).
    pub fn move_leaf<A: FrameAllocatorForWalk>(
        &mut self,
        src: VirtAddr,
        dst: VirtAddr,
        alloc: &A,
    ) -> Result<bool, AllocError> {
        if src.as_u64() == dst.as_u64() {
            return Ok(true);
        }

        let Some(src_entry_ptr) = self.leaf_entry_ptr(src) else {
            return Ok(false);
        };
        // SAFETY: the pointer comes from leaf_entry_ptr() and points to a 4 KiB PTE.
        let old_entry = unsafe { *src_entry_ptr };
        if !old_entry.is_present() {
            return Ok(false);
        }

        let page_flags = old_entry.to_page_flags();

        // SAFETY: pml4_phys is a valid PML4.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, dst.p4_index(), alloc, page_flags)?;
        // SAFETY: l3_phys points to a valid table.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, dst.p3_index(), alloc, page_flags)?;
        // SAFETY: l2_phys points to a valid table.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let l1_phys = self.ensure_table(pd, dst.p2_index(), alloc, page_flags)?;
        // SAFETY: l1_phys points to a valid table.
        let pt = unsafe { phys_to_table_mut(l1_phys) };

        let dst_entry = &mut pt[dst.p1_index()];
        if dst_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        *dst_entry = old_entry;
        // SAFETY: src_entry_ptr still points to the source PTE; intermediate
        // tables remain alive throughout the lifetime of the AS.
        unsafe {
            (*src_entry_ptr).clear();
        }
        Ok(true)
    }

    /// Reads the raw value of the 4 KiB leaf PTE for `virt`.
    /// Returns `0` if the page is not mapped or if a huge page is encountered.
    pub fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let Some(entry_ptr) = self.leaf_entry_ptr(virt) else {
            return 0;
        };
        // SAFETY: `entry_ptr` points to a valid 4 KiB PTE as long as the table exists.
        unsafe { (*entry_ptr).raw() }
    }

    /// Performs an atomic compare-and-swap on a 4 KiB leaf PTE.
    ///
    /// Returns `Err(actual_raw)` if the entry is no longer equal to `expected`,
    /// or `Err(0)` if no leaf PTE exists for this address.
    ///
    /// # Safety
    /// Accessing the PTE must respect page table synchronization invariants.
    pub unsafe fn compare_exchange_leaf_raw(
        &self,
        virt: VirtAddr,
        expected: u64,
        new: u64,
    ) -> Result<(), u64> {
        let Some(entry_ptr) = self.leaf_entry_ptr(virt) else {
            return Err(0);
        };

        let atomic_ptr = entry_ptr.cast::<AtomicU64>();
        match (*atomic_ptr).compare_exchange(expected, new, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(()),
            Err(actual) => Err(actual),
        }
    }

    // ─── helper: ensures an intermediate level exists ─────────────────────────

    fn leaf_entry_ptr(&self, virt: VirtAddr) -> Option<*mut PageTableEntry> {
        // SAFETY: `pml4_phys` references a valid hierarchy during the lifetime of the AS.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return None;
        }

        // SAFETY: the present entry points to a valid PDPT.
        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() || l3_entry.is_huge() {
            return None;
        }

        // SAFETY: the present entry points to a valid PD.
        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() || l2_entry.is_huge() {
            return None;
        }

        // SAFETY: the present entry points to a valid PT.
        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        Some((&mut pt[virt.p1_index()]) as *mut PageTableEntry)
    }

    fn ensure_table<A: FrameAllocatorForWalk>(
        &self,
        parent: &mut PageTable,
        idx: usize,
        alloc: &A,
        flags: PageFlags,
    ) -> Result<PhysAddr, AllocError> {
        let entry = &mut parent[idx];
        if entry.is_present() {
            // If the entry points to a huge page (2 MiB / 1 GiB),
            // it cannot be treated as an intermediate table.
            if entry.is_huge() {
                return Err(AllocError::InvalidParams);
            }
            if flags.contains(PageFlags::WRITABLE) {
                entry.set_flag(PageTableEntry::FLAG_WRITABLE);
            }
            if flags.contains(PageFlags::USER) {
                entry.set_flag(PageTableEntry::FLAG_USER);
            }
            return Ok(entry.phys_addr());
        }
        // Allocate a new frame for the intermediate table
        let new_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
        let new_phys = new_frame.start_address();
        // Initialize to zero (ZEROED flag guarantees 4 KiB at 0)
        // Write the entry with minimal rights (PRESENT | WRITABLE | USER if user)
        let parent_flags = PageTableEntry::FLAG_PRESENT
            | PageTableEntry::FLAG_WRITABLE
            | if flags.contains(PageFlags::USER) {
                PageTableEntry::FLAG_USER
            } else {
                0
            };
        *entry = PageTableEntry::from_raw(new_phys.as_u64() | parent_flags);
        Ok(new_phys)
    }

    #[inline]
    fn upgrade_table_entry_permissions(entry: &mut PageTableEntry, leaf_flags: PageFlags) {
        // U/S and R/W bits of intermediate levels restrict all
        // leaves in the subtree. The leaf maintains the fine-grained
        // read-only/NX policy; tables only need not block a legitimate
        // user or writable page further down.
        if leaf_flags.contains(PageFlags::WRITABLE) {
            entry.set_flag(PageTableEntry::FLAG_WRITABLE);
        }
        if leaf_flags.contains(PageFlags::USER) {
            entry.set_flag(PageTableEntry::FLAG_USER);
        }
    }
}
