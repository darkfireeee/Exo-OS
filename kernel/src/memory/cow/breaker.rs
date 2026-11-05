// kernel/src/memory/cow/breaker.rs
//
// CoW break (Copy-on-Write break) — physical copy of a shared frame.
//
// When a process attempts to write to a page marked `COW` and
// shared (refcount >= 2), the page fault handler calls `break_cow`.
// This function:
//   1. Allocates a new exclusive physical frame.
//   2. Copies the content of the old frame to the new one (physmap).
//   3. Decrements the refcount of the old frame via `COW_TRACKER`.
//   4. Returns the new frame (caller must update the PTE).
//
// If the frame is already exclusive (refcount == 1), copying is avoided
// and the same frame is returned after verification.
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::Ordering;

use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::layout::PHYS_MAP_BASE;
use crate::memory::core::types::{AllocError, AllocFlags, Frame};
use crate::memory::cow::tracker::COW_TRACKER;
use crate::memory::physical::allocator::{alloc_page, free_page};

// ─────────────────────────────────────────────────────────────────────────────
// CoW BREAK RESULT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of `break_cow`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CowBreakOutcome {
    /// New copy allocated. The returned frame replaces the old one in the PTE.
    Copied(Frame),
    /// Frame was already exclusive (refcount == 1) — no copy needed.
    /// The same frame can be used for writing directly.
    AlreadyExclusive(Frame),
    /// Allocation of new copy failed (OOM).
    Oom,
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::AtomicU64;

pub struct CowBreakerStats {
    /// CoW breaks performed (physical copy completed).
    pub breaks: AtomicU64,
    /// Frames that were already exclusive (zero copy).
    pub already_excl: AtomicU64,
    /// OOM failures during break.
    pub oom_failures: AtomicU64,
    /// Total bytes copied via CoW breaks.
    pub bytes_copied: AtomicU64,
}

impl Default for CowBreakerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl CowBreakerStats {
    const fn new() -> Self {
        CowBreakerStats {
            breaks: AtomicU64::new(0),
            already_excl: AtomicU64::new(0),
            oom_failures: AtomicU64::new(0),
            bytes_copied: AtomicU64::new(0),
        }
    }
}

pub static COW_BREAKER_STATS: CowBreakerStats = CowBreakerStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// CoW BREAK
// ─────────────────────────────────────────────────────────────────────────────

/// Breaks CoW sharing on `frame` and returns the resulting exclusive frame.
///
/// ## Algorithm
/// 1. Query `COW_TRACKER` for current refcount.
/// 2. If refcount <= 1 -> return `AlreadyExclusive(frame)`.
/// 3. Allocate a new frame via `alloc_page(AllocFlags::NONE)`.
/// 4. Copy PAGE_SIZE bytes from `frame` to the new one via physmap.
/// 5. Decrement the refcount of `frame` via `COW_TRACKER.dec(frame)`.
/// 6. Return `Copied(new_frame)`.
///
/// ## Post-conditions for the caller
/// - The old `frame` has its refcount decremented by one.
/// - If `COW_TRACKER.dec()` returns 0, free the old frame.
/// - Update the PTE to point to `new_frame`.
/// - Clear the `COW` bit in the PTE flags of this copy.
///
/// # Safety
/// - Both physical addresses (src and dst) must be in the physmap.
/// - The physmap (`PHYS_MAP_BASE`) must be initialized before calling.
pub unsafe fn break_cow(frame: Frame) -> CowBreakOutcome {
    let refcount = COW_TRACKER.ref_count(frame);

    // If the frame is already exclusive, no copy is necessary.
    if refcount <= 1 {
        COW_BREAKER_STATS
            .already_excl
            .fetch_add(1, Ordering::Relaxed);
        return CowBreakOutcome::AlreadyExclusive(frame);
    }

    // Allocate a new frame (non-zero — we will fill it immediately).
    let new_frame = match alloc_page(AllocFlags::NONE) {
        Ok(f) => f,
        Err(_) => {
            COW_BREAKER_STATS
                .oom_failures
                .fetch_add(1, Ordering::Relaxed);
            return CowBreakOutcome::Oom;
        }
    };

    // Copy content of old frame to new frame via direct physmap.
    let phys_base = PHYS_MAP_BASE.as_u64();
    let src = (phys_base + frame.phys_addr().as_u64()) as *const u8;
    let dst = (phys_base + new_frame.phys_addr().as_u64()) as *mut u8;

    // SAFETY: The physmap covers all RAM. PAGE_SIZE is fixed (4096).
    // Frames never overlap (each is at a distinct address).
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);

    // Decrement refcount of old frame.
    let new_rc = COW_TRACKER.dec(frame);
    if new_rc == 0 {
        // Last referrer gone — free old frame.
        let _ = free_page(frame);
    }

    COW_BREAKER_STATS.breaks.fetch_add(1, Ordering::Relaxed);
    COW_BREAKER_STATS
        .bytes_copied
        .fetch_add(PAGE_SIZE as u64, Ordering::Relaxed);

    CowBreakOutcome::Copied(new_frame)
}

/// "Best-effort" variant: on OOM, return an error without panicking.
/// Identical to `break_cow` but returns a `Result`.
///
/// # Safety
/// Same preconditions as `break_cow`.
#[inline]
pub unsafe fn try_break_cow(frame: Frame) -> Result<Frame, AllocError> {
    match break_cow(frame) {
        CowBreakOutcome::Copied(f) => Ok(f),
        CowBreakOutcome::AlreadyExclusive(f) => Ok(f),
        CowBreakOutcome::Oom => Err(AllocError::OutOfMemory),
    }
}
