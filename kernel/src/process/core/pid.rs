// kernel/src/process/core/pid.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PID / TID Allocator — lock-free radix bitmap allocator (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Design:
//   • Flat bitmap of 512 u64 words → 32,768 concurrent PIDs.
//   • Amortized O(1) allocation: CLZ + word-by-word scan.
//   • Separation of PID (processes) / TID (threads) via two independent allocators.
//   • PID_MIN = 1, PID_MAX = 32,767; TID_MAX = 131,071.
//   • PID 0 reserved (idle/swapper), PID 1 reserved (init).
//   • Thread-safe: atomic CAS on each bitmap word.
//   • Instrumentation: atomic alloc/free/current_count counters.
//
// RULE: No heap allocation in this file (partial NO-ALLOC zone).
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Process ID — non-zero, globally unique.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Pid(pub u32);

/// Thread ID — non-zero, globally unique.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Tid(pub u32);

impl Pid {
    pub const INVALID: Self = Self(u32::MAX);
    pub const IDLE: Self = Self(0);
    pub const INIT: Self = Self(1);
    pub const FIRST_USABLE: Self = Self(PID_FIRST_USABLE);

    #[inline(always)]
    pub fn is_valid(self) -> bool {
        self.0 != Self::INVALID.0 && self.0 <= PID_MAX
    }

    #[inline(always)]
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

impl Tid {
    pub const INVALID: Self = Self(u32::MAX);
    pub const FIRST_USABLE: Self = Self(TID_FIRST_USABLE);

    #[inline(always)]
    pub fn is_valid(self) -> bool {
        self.0 != Self::INVALID.0 && self.0 >= TID_FIRST_USABLE && self.0 <= TID_MAX
    }

    #[inline(always)]
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pid:{}", self.0)
    }
}
impl fmt::Display for Tid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tid:{}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum capacity of the PID bitmap (entries).
pub const PID_BITMAP_WORDS: usize = 512; // 512 × 64 = 32,768 PIDs
pub const TID_BITMAP_WORDS: usize = 2048; // 2048 × 64 = 131,072 TIDs

pub const PID_FIRST_USABLE: u32 = 2; // 0=idle, 1=init reserved
pub const PID_MAX: u32 = (PID_BITMAP_WORDS as u32 * 64) - 1;
pub const TID_FIRST_USABLE: u32 = 1;
pub const TID_MAX: u32 = (TID_BITMAP_WORDS as u32 * 64) - 1;

// ─────────────────────────────────────────────────────────────────────────────
// PidBitmap — static bitmap structure
// ─────────────────────────────────────────────────────────────────────────────

/// Flat bitmap of N AtomicU64 words.
/// Bit set to 1 = ID FREE, bit set to 0 = ID USED.
struct PidBitmap<const N: usize> {
    words: [AtomicU64; N],
}

impl<const N: usize> PidBitmap<N> {
    const fn new_all_free() -> Self {
        // SAFETY: AtomicU64::new(u64::MAX) is a valid constant.
        // All bits set to 1 = all free.
        #[allow(clippy::declare_interior_mutable_const)]
        const WORD_FREE: AtomicU64 = AtomicU64::new(u64::MAX);
        Self {
            words: [WORD_FREE; N],
        }
    }

    /// Allocates the first free ID >= `first_usable`.
    /// Returns `None` if exhausted.
    /// Algorithm: word-by-word scan, CLZ to find a free bit.
    fn alloc(&self, first_usable: u32) -> Option<u32> {
        let start_word = (first_usable / 64) as usize;
        let total_words = N;

        for w in start_word..total_words {
            let val = self.words[w].load(Ordering::Relaxed);
            if val == 0 {
                continue;
            } // word full

            // Find least significant free bit.
            let bit = val.trailing_zeros(); // position of first bit set to 1
            let mask = 1u64 << bit;

            // Attempt to mark as used (1→0) with CAS.
            if self.words[w]
                .compare_exchange(val, val & !mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let id = (w as u32) * 64 + bit;
                if id >= first_usable {
                    return Some(id);
                }
                // ID too low (< first_usable) — release it and continue.
                self.words[w].fetch_or(mask, Ordering::Relaxed);
                continue;
            }
            // CAS failed (race) — retry this word.
            // For simplicity, move to the next word (next allocation will succeed).
        }
        None
    }

    /// Frees the given ID (resets bit to 1).
    /// Panics (debug) if the ID was already free.
    fn free(&self, id: u32) {
        let w = (id / 64) as usize;
        let bit = id % 64;
        let mask = 1u64 << bit;
        let prev = self.words[w].fetch_or(mask, Ordering::Release);
        debug_assert!(
            prev & mask == 0,
            "PidBitmap::free: id {} était déjà libre (double free)",
            id
        );
    }

    /// Checks whether the ID is used (bit set to 0).
    fn is_used(&self, id: u32) -> bool {
        let w = (id / 64) as usize;
        let bit = id % 64;
        self.words[w].load(Ordering::Relaxed) & (1u64 << bit) == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PidAllocator — wrapper with statistics counters
// ─────────────────────────────────────────────────────────────────────────────

/// PID allocator with full instrumentation.
pub struct PidAllocator {
    bitmap: &'static PidBitmapPid,
    first_usable: u32,
    #[allow(dead_code)]
    total_capacity: u32,
    /// Number of successful allocations since boot.
    alloc_count: AtomicU64,
    /// Number of frees since boot.
    free_count: AtomicU64,
    /// Current number of used IDs.
    current_used: AtomicU32,
    /// Maximum number of concurrently used IDs (high-water mark).
    peak_used: AtomicU32,
    /// Number of allocation failures (exhaustion).
    exhausted: AtomicU64,
}

// Type alias for static bitmaps.
type PidBitmapPid = PidBitmap<PID_BITMAP_WORDS>;
type PidBitmapTid = PidBitmap<TID_BITMAP_WORDS>;

static PID_BITMAP_STORAGE: PidBitmapPid = PidBitmap::new_all_free();
static TID_BITMAP_STORAGE: PidBitmapTid = PidBitmap::new_all_free();

// SAFETY: PidBitmap only accesses its fields via atomics.
unsafe impl Sync for PidBitmapPid {}
unsafe impl Sync for PidBitmapTid {}

pub static PID_ALLOCATOR: PidAllocator = PidAllocator {
    // SAFETY: PID_BITMAP_STORAGE is an aligned static array with correct layout;
    //         PidBitmapPid is a transparent newtype around the same underlying type.
    bitmap: unsafe { &*(&PID_BITMAP_STORAGE as *const PidBitmapPid) },
    first_usable: PID_FIRST_USABLE,
    total_capacity: PID_MAX,
    alloc_count: AtomicU64::new(0),
    free_count: AtomicU64::new(0),
    current_used: AtomicU32::new(0),
    peak_used: AtomicU32::new(0),
    exhausted: AtomicU64::new(0),
};

pub static TID_ALLOCATOR: PidAllocator = PidAllocator {
    // SAFETY: TID_BITMAP_STORAGE is an aligned static array; cast to PidBitmapPid is valid
    //         because PidBitmapTid and PidBitmapPid share the same layout (same underlying type).
    bitmap: unsafe { &*(&TID_BITMAP_STORAGE as *const PidBitmapTid as *const PidBitmapPid) },
    first_usable: TID_FIRST_USABLE,
    total_capacity: TID_MAX,
    alloc_count: AtomicU64::new(0),
    free_count: AtomicU64::new(0),
    current_used: AtomicU32::new(0),
    peak_used: AtomicU32::new(0),
    exhausted: AtomicU64::new(0),
};

// SAFETY: PidAllocator only accesses its atomic fields via Atomic*.
unsafe impl Sync for PidAllocator {}
unsafe impl Send for PidAllocator {}

/// PID/TID allocation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidAllocError {
    /// No more IDs available.
    Exhausted,
    /// Requested ID is already in use (explicit reservation only).
    AlreadyUsed,
}

impl PidAllocator {
    /// Allocates a new PID/TID.
    /// Returns `Err(Exhausted)` if all IDs are in use.
    #[inline]
    pub fn alloc(&self) -> Result<u32, PidAllocError> {
        match self.bitmap.alloc(self.first_usable) {
            Some(id) => {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                let cur = self.current_used.fetch_add(1, Ordering::Relaxed) + 1;
                // Lock-free update of high-water mark.
                let mut peak = self.peak_used.load(Ordering::Relaxed);
                while cur > peak {
                    match self.peak_used.compare_exchange_weak(
                        peak,
                        cur,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(p) => peak = p,
                    }
                }
                Ok(id)
            }
            None => {
                self.exhausted.fetch_add(1, Ordering::Relaxed);
                Err(PidAllocError::Exhausted)
            }
        }
    }

    /// Frees a previously allocated PID/TID.
    #[inline]
    pub fn free(&self, id: u32) {
        self.bitmap.free(id);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        self.current_used.fetch_sub(1, Ordering::Relaxed);
    }

    /// Checks if an ID is currently in use.
    #[inline(always)]
    pub fn is_used(&self, id: u32) -> bool {
        self.bitmap.is_used(id)
    }

    /// Number of IDs currently in use.
    #[inline(always)]
    pub fn current_count(&self) -> u32 {
        self.current_used.load(Ordering::Relaxed)
    }

    /// Total number of allocations since boot.
    #[inline(always)]
    pub fn total_allocs(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    /// Number of allocation failures (exhaustion).
    #[inline(always)]
    pub fn exhaustion_count(&self) -> u64 {
        self.exhausted.load(Ordering::Relaxed)
    }

    /// Concurrent usage high-water mark.
    #[inline(always)]
    pub fn peak_count(&self) -> u32 {
        self.peak_used.load(Ordering::Relaxed)
    }
}

/// Initializes PID and TID allocators.
/// Reserves PID 0 (idle) and PID 1 (init) as used.
///
/// # Safety
/// Called once from the BSP at boot.
pub unsafe fn init(_max_pids: usize, _max_tids: usize) {
    // Reserve PID 0 = idle.
    PID_BITMAP_STORAGE.words[0].fetch_and(!(1u64 << 0), Ordering::Relaxed);
    // Reserve PID 1 = init.
    PID_BITMAP_STORAGE.words[0].fetch_and(!(1u64 << 1), Ordering::Relaxed);
    PID_ALLOCATOR.current_used.store(2, Ordering::Relaxed);
}
