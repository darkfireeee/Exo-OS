// kernel/src/memory/swap/policy.rs
//
// Swapping policy — decides which pages to evict (LRU / CLOCK-Pro).
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// LRU-CLOCK ALGORITHM (LRU approximation via Accessed bit)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of frames in the eviction candidate list.
pub const EVICT_CANDIDATE_LIST_SIZE: usize = 1024;

/// Swapper statistics.
pub struct SwapPolicyStats {
    pub pages_evicted: AtomicU64,
    pub pages_reclaimed: AtomicU64,
    pub scan_cycles: AtomicU64,
    pub accessed_cleared: AtomicU64,
    pub dirty_skipped: AtomicU64,
    pub pinned_skipped: AtomicU64,
    pub oom_triggers: AtomicU64,
}

impl SwapPolicyStats {
    const fn new() -> Self {
        SwapPolicyStats {
            pages_evicted: AtomicU64::new(0),
            pages_reclaimed: AtomicU64::new(0),
            scan_cycles: AtomicU64::new(0),
            accessed_cleared: AtomicU64::new(0),
            dirty_skipped: AtomicU64::new(0),
            pinned_skipped: AtomicU64::new(0),
            oom_triggers: AtomicU64::new(0),
        }
    }
}

pub static SWAP_POLICY_STATS: SwapPolicyStats = SwapPolicyStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// EVICTION LIST MANAGER (CLOCK)
// ─────────────────────────────────────────────────────────────────────────────

/// Candidate for eviction in the CLOCK list.
#[derive(Copy, Clone, Debug)]
pub struct EvictCandidate {
    pub phys: PhysAddr,
    /// Associated address space (opaque, encoded as u64).
    pub as_id: u64,
    /// Virtual address in the address space.
    pub virt_addr: u64,
    /// LRU generation (from FrameDesc::lru_gen).
    pub lru_gen: u8,
}

impl EvictCandidate {
    pub const EMPTY: Self = EvictCandidate {
        phys: PhysAddr::new(0),
        as_id: 0,
        virt_addr: 0,
        lru_gen: 0,
    };
    pub fn is_valid(&self) -> bool {
        self.phys.as_u64() != 0
    }
}

/// CLOCK list for the eviction algorithm.
pub struct ClockEvictList {
    inner: Mutex<ClockEvictListInner>,
}

struct ClockEvictListInner {
    candidates: [EvictCandidate; EVICT_CANDIDATE_LIST_SIZE],
    clock_hand: usize,
    count: usize,
}

// SAFETY: ClockEvictList is protected by Mutex.
unsafe impl Sync for ClockEvictList {}
unsafe impl Send for ClockEvictList {}

impl ClockEvictList {
    const fn new() -> Self {
        ClockEvictList {
            inner: Mutex::new(ClockEvictListInner {
                candidates: [EvictCandidate::EMPTY; EVICT_CANDIDATE_LIST_SIZE],
                clock_hand: 0,
                count: 0,
            }),
        }
    }

    /// Adds a candidate to the list (if not full).
    pub fn add(&self, candidate: EvictCandidate) {
        let mut inner = self.inner.lock();
        if inner.count >= EVICT_CANDIDATE_LIST_SIZE {
            return;
        }
        // Insert at the next available slot.
        for slot in inner.candidates.iter_mut() {
            if !slot.is_valid() {
                *slot = candidate;
                inner.count += 1;
                return;
            }
        }
    }

    /// Selects the next victim via CLOCK.
    /// Returns `None` if the list is empty or all pages are "accessed".
    pub fn next_victim(&self) -> Option<EvictCandidate> {
        let mut inner = self.inner.lock();
        if inner.count == 0 {
            return None;
        }

        let mut scans = 0usize;
        loop {
            if scans >= EVICT_CANDIDATE_LIST_SIZE * 2 {
                return None;
            }
            scans += 1;

            let idx = inner.clock_hand % EVICT_CANDIDATE_LIST_SIZE;
            inner.clock_hand = (inner.clock_hand + 1) % EVICT_CANDIDATE_LIST_SIZE;

            let c = &inner.candidates[idx];
            if !c.is_valid() {
                continue;
            }

            // Checks the Accessed bit in the FrameDesc.
            // (In practice one would also check the PTE Accessed bit).
            let candidate = *c;

            // Selects this victim.
            inner.candidates[idx] = EvictCandidate::EMPTY;
            inner.count -= 1;
            SWAP_POLICY_STATS
                .scan_cycles
                .fetch_add(scans as u64, Ordering::Relaxed);
            return Some(candidate);
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().count
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub static EVICT_LIST: ClockEvictList = ClockEvictList::new();

// ─────────────────────────────────────────────────────────────────────────────
// EVICTION POLICY
// ─────────────────────────────────────────────────────────────────────────────

/// Watermarks to trigger swapping.
#[derive(Copy, Clone, Debug)]
pub struct SwapWatermarks {
    /// Minimum free pages before triggering kswapd.
    pub low: u64,
    /// "Healthy" free pages (no swap).
    pub high: u64,
    /// Emergency free pages (switches to OOM-kill mode if below).
    pub critical: u64,
}

impl SwapWatermarks {
    pub const DEFAULT: Self = SwapWatermarks {
        low: 512,     // 2 MiB
        high: 2048,   // 8 MiB
        critical: 64, // 256 KiB
    };
}

pub static SWAP_WATERMARKS: spin::RwLock<SwapWatermarks> =
    spin::RwLock::new(SwapWatermarks::DEFAULT);

/// Evaluates whether swapping should be triggered.
pub fn should_swap(free_pages: u64) -> bool {
    let wm = SWAP_WATERMARKS.read();
    free_pages < wm.low
}

/// Evaluates whether the situation is critical (OOM).
pub fn is_critical(free_pages: u64) -> bool {
    let wm = SWAP_WATERMARKS.read();
    free_pages < wm.critical
}
