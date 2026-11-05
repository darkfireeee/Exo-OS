// kernel/src/memory/virtual/vma/cow.rs
//
// Copy-on-Write for VMAs — handling CoW breaks and inheritance on fork.
// Layer 0 — no external dependency except `spin`.

use super::descriptor::{VmaDescriptor, VmaFlags};
use crate::memory::core::{AllocError, Frame, PageFlags, PAGE_SIZE};
use crate::memory::physical::frame::ref_count::{AtomicRefCount, RefCountDecResult};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// COW STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct CowStats {
    pub breaks: AtomicU64,
    pub shared_pages: AtomicU64,
    pub zero_pages: AtomicU64,
    pub fork_copies: AtomicU64,
}

impl Default for CowStats {
    fn default() -> Self {
        Self::new()
    }
}

impl CowStats {
    pub const fn new() -> Self {
        CowStats {
            breaks: AtomicU64::new(0),
            shared_pages: AtomicU64::new(0),
            zero_pages: AtomicU64::new(0),
            fork_copies: AtomicU64::new(0),
        }
    }
}

pub static COW_STATS: CowStats = CowStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATION TRAIT FOR COW
// ─────────────────────────────────────────────────────────────────────────────

/// Trait allowing the CoW module to allocate/free frames.
pub trait CowFrameAllocator: Sync {
    fn alloc_zeroed(&self) -> Result<Frame, AllocError>;
    fn alloc_nonzeroed(&self) -> Result<Frame, AllocError>;
    fn free_frame(&self, f: Frame);
}

// ─────────────────────────────────────────────────────────────────────────────
// COW BREAK
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a CoW break for a page.
pub enum CowBreakResult {
    /// Page had refcount=1 (exclusive) — nothing to copy,
    /// simply mark the page writable.
    AlreadyExclusive(Frame),
    /// A copy was performed — new frame.
    Copied(Frame),
    /// The source frame was zero_page — allocate a zeroed page.
    ZeroPage(Frame),
    /// Allocation error.
    Error(AllocError),
}

/// Performs a CoW break for `old_frame`.
///
/// # Safety
/// `old_frame` must have been pinned beforehand to avoid
/// concurrent release. The TLB must be invalidated by the caller.
pub unsafe fn cow_break<A: CowFrameAllocator>(
    old_frame: Frame,
    refcount: &AtomicRefCount,
    alloc: &A,
) -> CowBreakResult {
    // Attempt to decrement: if refcount becomes 1, the frame becomes exclusive.
    match refcount.dec() {
        RefCountDecResult::BecameExclusive => {
            // Frame is now exclusive — no more CoW, just set back to W.
            COW_STATS.breaks.fetch_add(1, Ordering::Relaxed);
            CowBreakResult::AlreadyExclusive(old_frame)
        }
        RefCountDecResult::StillShared => {
            // Shared: allocate a new frame and copy contents.
            let new_frame = match alloc.alloc_nonzeroed() {
                Ok(f) => f,
                Err(e) => {
                    // Restore refcount to its previous state.
                    refcount.inc();
                    return CowBreakResult::Error(e);
                }
            };
            // Copy contents from old frame to new frame.
            let src = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
                + old_frame.start_address().as_u64()) as *const u8;
            let dst = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
                + new_frame.start_address().as_u64()) as *mut u8;
            // SAFETY: src and dst are valid physical ranges mapped
            //         in the physmap. Size = PAGE_SIZE bytes.
            core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
            COW_STATS.breaks.fetch_add(1, Ordering::Relaxed);
            COW_STATS.fork_copies.fetch_add(1, Ordering::Relaxed);
            CowBreakResult::Copied(new_frame)
        }
        RefCountDecResult::ShouldFree => {
            // Last referrer — free the frame and take the new one.
            // (This case should not happen if breaking on refcount >= 2,
            //  but handled for robustness.)
            alloc.free_frame(old_frame);
            match alloc.alloc_zeroed() {
                Ok(f) => {
                    COW_STATS.zero_pages.fetch_add(1, Ordering::Relaxed);
                    CowBreakResult::ZeroPage(f)
                }
                Err(e) => CowBreakResult::Error(e),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FORK: MARK A VMA AS COW
// ─────────────────────────────────────────────────────────────────────────────

/// On fork, mark a VMA as CoW (disable writes in
/// parent and child page tables — break will occur on first write).
///
/// - Clears the WRITABLE flag from parent PT entries.
/// - Adds the COW flag to parent PT entries.
/// - Child will inherit the same entries (PML4 copy handled elsewhere).
pub fn mark_vma_cow(vma: &mut VmaDescriptor) {
    // The VMA is only writable via the PageTableWalker trait
    // which is called by the fault handler. Here we only mark
    // the descriptor flags — the table walk will be done by address_space::fork().
    if vma.flags.contains(VmaFlags::WRITE) && !vma.flags.contains(VmaFlags::SHARED) {
        vma.flags |= VmaFlags::COW;
        vma.page_flags = vma.page_flags & !PageFlags::WRITABLE | PageFlags::COW;
    }
    COW_STATS
        .shared_pages
        .fetch_add(vma.n_pages() as u64, Ordering::Relaxed);
}
