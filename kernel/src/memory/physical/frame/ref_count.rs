// kernel/src/memory/physical/frame/ref_count.rs
//
// Atomic CoW refcount for physical frames.
// Handles memory sharing scenarios (fork(), mmap(), SHM).
// Layer 0 — no external dependencies.

use core::sync::atomic::{AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ATOMIC REFCOUNT
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic reference counter for physical frame.
///
/// Protocol:
///   - 0 = free frame (no owner)
///   - 1 = exclusively allocated frame (read/write access)
///   - 2+ = CoW shared frame (read-only, copy-on-write)
///
/// Thread-safety: all operations are atomic and lock-free.
#[repr(transparent)]
pub struct AtomicRefCount(AtomicU32);

impl AtomicRefCount {
    /// Creates a refcount at zero (free frame).
    #[inline(always)]
    pub const fn new_zero() -> Self {
        AtomicRefCount(AtomicU32::new(0))
    }

    /// Creates a refcount at 1 (exclusively allocated frame).
    #[inline(always)]
    pub const fn new_one() -> Self {
        AtomicRefCount(AtomicU32::new(1))
    }

    /// Returns the current value (snapshot; may change afterwards).
    #[inline(always)]
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Checks if frame is free (refcount == 0).
    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0.load(Ordering::Acquire) == 0
    }

    /// Checks if frame is exclusive (refcount == 1).
    #[inline(always)]
    pub fn is_exclusive(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }

    /// Checks if frame is shared (refcount >= 2).
    #[inline(always)]
    pub fn is_shared(&self) -> bool {
        self.0.load(Ordering::Acquire) >= 2
    }

    /// Increments refcount, returns new value.
    /// Cannot overflow (saturates at u32::MAX, debug asserts).
    #[inline(always)]
    pub fn inc(&self) -> u32 {
        let mut cur = self.0.load(Ordering::Acquire);
        loop {
            if cur == u32::MAX {
                debug_assert!(false, "AtomicRefCount overflow");
                return u32::MAX;
            }
            let next = cur + 1;
            match self
                .0
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return next,
                Err(observed) => cur = observed,
            }
        }
    }

    /// Decrements refcount.
    /// Returns `RefCountDecResult` indicating required action.
    ///
    /// # Double-free guard (release + debug)
    /// If refcount is already 0, stops decrement (restores 0) and
    /// returns `StillShared` to avoid improper release.
    #[inline]
    pub fn dec(&self) -> RefCountDecResult {
        let mut cur = self.0.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                debug_assert!(false, "AtomicRefCount underflow — double-free détecté");
                return RefCountDecResult::StillShared;
            }
            let next = cur - 1;
            match self
                .0
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return match cur {
                        1 => RefCountDecResult::ShouldFree,
                        2 => RefCountDecResult::BecameExclusive,
                        _ => RefCountDecResult::StillShared,
                    };
                }
                Err(observed) => cur = observed,
            }
        }
    }

    /// Attempts to increment refcount only if non-zero (live frame).
    /// "Get-or-create" pattern: useful for lookups in the frame table.
    /// Returns `false` if frame was free (refcount == 0).
    #[inline]
    pub fn try_inc_if_nonzero(&self) -> bool {
        let mut current = self.0.load(Ordering::Relaxed);
        loop {
            if current == 0 {
                return false;
            }
            match self.0.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new) => current = new,
            }
        }
    }

    /// Sets refcount to 0 (forced release — boot/reinit only).
    ///
    /// # Safety
    /// No other thread may be using this frame at the time of call.
    #[inline(always)]
    pub unsafe fn reset_to_zero(&self) {
        self.0.store(0, Ordering::Release);
    }

    /// Sets refcount to 1 (initial allocation — boot/reinit only).
    ///
    /// # Safety
    /// Same as `reset_to_zero`.
    #[inline(always)]
    pub unsafe fn reset_to_one(&self) {
        self.0.store(1, Ordering::Release);
    }

    /// Attempts a CAS from `expected` to `new`.
    /// Returns Ok(()) on success, Err(current) otherwise.
    #[inline]
    pub fn compare_exchange(&self, expected: u32, new: u32) -> Result<(), u32> {
        self.0
            .compare_exchange(expected, new, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
    }
}

/// Result of a refcount decrement.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RefCountDecResult {
    /// Refcount dropped to 0 — frame must be freed (returned to buddy).
    ShouldFree,
    /// Refcount dropped to 1 — last owner can write without CoW.
    BecameExclusive,
    /// Refcount is still >= 2 — frame remains shared (CoW still active).
    StillShared,
}

// ─────────────────────────────────────────────────────────────────────────────
// COW SLOT — entry in the CoW table
// ─────────────────────────────────────────────────────────────────────────────

/// CoW tracking entry for a frame.
/// Tracks how many processes share this frame for reading.
#[repr(C, align(16))]
pub struct CowSlot {
    /// PFN of shared frame.
    pub pfn: u64,
    /// CoW refcount (expected copies).
    pub refcount: AtomicRefCount,
    /// Padding to align to 16 bytes.
    _pad: u32,
}

const _: () = assert!(
    core::mem::size_of::<CowSlot>() == 16,
    "CowSlot doit faire 16 bytes"
);

impl CowSlot {
    #[inline]
    pub const fn new(pfn: u64) -> Self {
        CowSlot {
            pfn,
            refcount: AtomicRefCount::new_one(),
            _pad: 0,
        }
    }

    /// Checks if this slot is free (pfn == 0).
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.pfn == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COW BREAK — copy-on-write logic
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a CoW break.
#[derive(Debug)]
pub enum CowBreakResult {
    /// Page was exclusive — no copy needed, direct write OK.
    AlreadyExclusive,
    /// CoW break succeeded — new physical address (copy of original frame).
    Copied { new_frame_pfn: u64 },
    /// Failure: out of memory for the copy.
    OutOfMemory,
}

/// Checks if a CoW frame can be promoted to exclusive without copying.
/// Condition: refcount == 1 after shared reference decrement.
///
/// Usage: on Ring 3 CoW page fault, page fault handler
/// calls this function before deciding whether to copy.
#[inline]
pub fn cow_can_promote(refcount: &AtomicRefCount) -> bool {
    // If refcount is already 1, this thread is the sole holder
    refcount.is_exclusive()
}
