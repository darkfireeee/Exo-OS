// kernel/src/memory/physical/frame/reclaim.rs
//
// Frame reclamation — two-handed CLOCK algorithm implementation
// (active list / inactive list).
//
// Architecture:
//   • LRU inactive list — swap candidates.
//   • LRU active   list — hot frames, not yet candidates.
//   • `kswapd_reclaim(target)` — attempts to free `target` frames.
//   • Per-CPU flag `PF_MEMALLOC` — indicates thread is already
//     reclaiming (prevents recursion).
//   • `lru_add_new` / `lru_promote` / `lru_demote` — list management.
//
// Simplified CLOCK-pro:
//   1. Scan `inactive` from the clock hand.
//   2. ACCESSED frame -> promote to `active`, clear ACCESSED bit.
//   3. Non-ACCESSED frame -> candidate for swap or direct release.
//   4. PINNED / DMA frame -> skip.
//   5. When `active` overflows (> HIGH_WATER_ACTIVE), demote to `inactive`.
//
// Calibrated constants for a no_std kernel OS.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::constants::MAX_CPUS;
use crate::memory::core::types::{Frame, PhysAddr};
use crate::memory::physical::allocator::free_page;
use crate::memory::physical::frame::descriptor::{FrameFlags, FRAME_DESCRIPTORS};
use crate::memory::swap::backend::SWAP_BACKEND;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum size of each LRU list (in number of frames).
pub const LRU_LIST_SIZE: usize = 8192;

/// Threshold beyond which active -> inactive frames demotion begins.
pub const HIGH_WATER_ACTIVE: usize = LRU_LIST_SIZE * 3 / 4;

/// Size of RECLAIM_FLAGS table.
///
/// Aligned on the canonical constant to avoid a reclaim table wider
/// than the actual number of CPUs supported by the kernel.
const RECLAIM_MAX_CPUS: usize = MAX_CPUS;
const _: () = assert!(
    RECLAIM_MAX_CPUS >= MAX_CPUS,
    "RECLAIM_MAX_CPUS doit rester >= MAX_CPUS canonique"
);

/// Maximum number of frames to write to swap per reclaim pass.
pub const MAX_SWAP_PER_PASS: usize = 64;

/// PF_MEMALLOC bit in per-CPU flags (bit 0).
const PF_MEMALLOC_BIT: u32 = 1 << 0;
/// PF_KSWAPD bit in per-CPU flags (bit 1).
#[allow(dead_code)]
const PF_KSWAPD_BIT: u32 = 1 << 1;

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU FLAGS
// ─────────────────────────────────────────────────────────────────────────────

/// Per-CPU flags table for reclaim threads.
static RECLAIM_FLAGS: [AtomicU32; RECLAIM_MAX_CPUS] =
    [const { AtomicU32::new(0) }; RECLAIM_MAX_CPUS];

/// Marks current CPU as performing reclaim (antirecursion).
#[inline]
pub fn enter_memalloc(cpu_id: usize) {
    if cpu_id < RECLAIM_MAX_CPUS {
        RECLAIM_FLAGS[cpu_id].fetch_or(PF_MEMALLOC_BIT, Ordering::Relaxed);
    }
}

/// Clears PF_MEMALLOC marker on current CPU.
#[inline]
pub fn leave_memalloc(cpu_id: usize) {
    if cpu_id < RECLAIM_MAX_CPUS {
        RECLAIM_FLAGS[cpu_id].fetch_and(!PF_MEMALLOC_BIT, Ordering::Relaxed);
    }
}

/// Returns `true` if current CPU is currently reclaiming.
#[inline]
pub fn in_memalloc(cpu_id: usize) -> bool {
    if cpu_id >= RECLAIM_MAX_CPUS {
        return false;
    }
    RECLAIM_FLAGS[cpu_id].load(Ordering::Relaxed) & PF_MEMALLOC_BIT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// LRU LISTS (STATIC RING-BUFFER)
// ─────────────────────────────────────────────────────────────────────────────

/// LRU list entry.
#[derive(Copy, Clone)]
struct LruEntry {
    pfn: u64,
    valid: bool,
}

impl LruEntry {
    const EMPTY: LruEntry = LruEntry {
        pfn: 0,
        valid: false,
    };
}

struct LruList {
    entries: [LruEntry; LRU_LIST_SIZE],
    head: usize, // next slot to insert
    tail: usize, // next slot to remove (FIFO)
    count: usize,
    /// Clock pointer (CLOCK scan).
    clock: usize,
}

impl LruList {
    const fn new() -> Self {
        LruList {
            entries: [LruEntry::EMPTY; LRU_LIST_SIZE],
            head: 0,
            tail: 0,
            count: 0,
            clock: 0,
        }
    }

    fn push(&mut self, pfn: u64) -> bool {
        if self.count >= LRU_LIST_SIZE {
            return false;
        }
        self.entries[self.head] = LruEntry { pfn, valid: true };
        self.head = (self.head + 1) & (LRU_LIST_SIZE - 1);
        self.count += 1;
        true
    }

    /// Removes next valid entry from queue (FIFO).
    fn pop(&mut self) -> Option<u64> {
        while self.count > 0 {
            let e = self.entries[self.tail];
            self.entries[self.tail] = LruEntry::EMPTY;
            self.tail = (self.tail + 1) & (LRU_LIST_SIZE - 1);
            self.count -= 1;
            if e.valid {
                return Some(e.pfn);
            }
        }
        None
    }

    /// Marks entry `pfn` as invalid (explicit removal).
    fn remove(&mut self, pfn: u64) {
        for e in self.entries.iter_mut() {
            if e.valid && e.pfn == pfn {
                e.valid = false;
                if self.count > 0 {
                    self.count -= 1;
                }
                break;
            }
        }
    }

    /// Advances clock pointer and returns next valid entry.
    fn clock_next(&mut self) -> Option<u64> {
        let start = self.clock;
        loop {
            let idx = self.clock;
            self.clock = (self.clock + 1) & (LRU_LIST_SIZE - 1);
            if self.entries[idx].valid {
                return Some(self.entries[idx].pfn);
            }
            if self.clock == start {
                break;
            }
        }
        None
    }

    #[inline]
    fn len(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MUTEX-PROTECTED STATICS
// ─────────────────────────────────────────────────────────────────────────────

static ACTIVE_LIST: Mutex<LruList> = Mutex::new(LruList::new());
static INACTIVE_LIST: Mutex<LruList> = Mutex::new(LruList::new());

// ─────────────────────────────────────────────────────────────────────────────
// RECLAIM STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct ReclaimStats {
    /// Directly freed frames (frame_flags: FREE / non-DIRTY).
    pub freed_direct: AtomicU64,
    /// Frames swapped out then freed.
    pub swapped_out: AtomicU64,
    /// Frames promoted from inactive -> active (ACCESSED bit).
    pub promoted: AtomicU64,
    /// Frames demoted from active -> inactive.
    pub demoted: AtomicU64,
    /// Skipped frames (PINNED / DMA).
    pub skipped: AtomicU64,
    /// Number of times swap failed.
    pub swap_errors: AtomicU64,
}

impl ReclaimStats {
    const fn new() -> Self {
        ReclaimStats {
            freed_direct: AtomicU64::new(0),
            swapped_out: AtomicU64::new(0),
            promoted: AtomicU64::new(0),
            demoted: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            swap_errors: AtomicU64::new(0),
        }
    }
}

pub static RECLAIM_STATS: ReclaimStats = ReclaimStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// RECLAIM RESULT
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
pub struct ReclaimResult {
    pub freed: usize,
    pub swapped: usize,
    pub promoted: usize,
    pub skipped: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// LRU PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Adds a new frame to the inactive list (new -> inactive by default).
pub fn lru_add_new(frame: Frame) {
    let mut inactive = INACTIVE_LIST.lock();
    if !inactive.push(frame.pfn()) {
        // List full — trigger mini-reclaim.
        drop(inactive);
        let _ = kswapd_reclaim(8, usize::MAX);
        let _ = INACTIVE_LIST.lock().push(frame.pfn());
    }
}

/// Removes a frame from both LRU lists (on `free_page`).
pub fn lru_remove(frame: Frame) {
    ACTIVE_LIST.lock().remove(frame.pfn());
    INACTIVE_LIST.lock().remove(frame.pfn());
}

/// Promotes a frame from inactive -> active (access page fault).
pub fn promote_to_active(frame: Frame) {
    let pfn = frame.pfn();
    {
        let mut inactive = INACTIVE_LIST.lock();
        inactive.remove(pfn);
    }
    let pushed = ACTIVE_LIST.lock().push(pfn);
    if pushed {
        RECLAIM_STATS.promoted.fetch_add(1, Ordering::Relaxed);
    }
    // If active overflows, demote oldest to inactive.
    if ACTIVE_LIST.lock().len() > HIGH_WATER_ACTIVE {
        let victim = ACTIVE_LIST.lock().pop();
        if let Some(v) = victim {
            let _ = INACTIVE_LIST.lock().push(v);
            RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Demotes a frame from active -> inactive.
pub fn demote_to_inactive(frame: Frame) {
    let pfn = frame.pfn();
    ACTIVE_LIST.lock().remove(pfn);
    let _ = INACTIVE_LIST.lock().push(pfn);
    RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// FRAME HELPERS
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn frame_from_pfn(pfn: u64) -> Frame {
    Frame::containing(PhysAddr::new(pfn << 12))
}

#[inline]
fn frame_is_reclaimable(pfn: u64) -> bool {
    let frame = frame_from_pfn(pfn);
    let desc = FRAME_DESCRIPTORS.get(frame);
    let flags = desc.flags();
    // Ignore system frames (PINNED) or DMA.
    !flags.contains(FrameFlags::PINNED) && !flags.contains(FrameFlags::DMA)
}

#[inline]
fn frame_is_accessed(pfn: u64) -> bool {
    let frame = frame_from_pfn(pfn);
    FRAME_DESCRIPTORS
        .get(frame)
        .flags()
        .contains(FrameFlags::ACCESSED)
}

#[inline]
fn frame_clear_accessed(pfn: u64) {
    let frame = frame_from_pfn(pfn);
    FRAME_DESCRIPTORS
        .get(frame)
        .clear_flag(FrameFlags::ACCESSED);
}

// ─────────────────────────────────────────────────────────────────────────────
// KSWAPD RECLAIM
// ─────────────────────────────────────────────────────────────────────────────

/// Core reclaim algorithm.
///
/// Attempts to free `target` frames by scanning the inactive list.
/// `cpu_id` is the identifier of calling CPU (for PF_MEMALLOC).
///
/// ## Single-handed CLOCK algorithm on inactive list
/// 1. Take next candidate via `clock_next()`.
/// 2. If frame is ACCESSED -> promote to active, continue.
/// 3. If frame is PINNED / DMA -> skip.
/// 4. If frame is DIRTY -> swap out then free.
/// 5. Otherwise -> free directly.
pub fn kswapd_reclaim(target: usize, cpu_id: usize) -> ReclaimResult {
    let mut result = ReclaimResult {
        freed: 0,
        swapped: 0,
        promoted: 0,
        skipped: 0,
    };

    if cpu_id < RECLAIM_MAX_CPUS && in_memalloc(cpu_id) {
        // Recursion detected — do not reclaim.
        return result;
    }

    if cpu_id < RECLAIM_MAX_CPUS {
        enter_memalloc(cpu_id);
    }

    let mut passes = 0usize;
    let max_scan = LRU_LIST_SIZE.min(target * 4);

    while result.freed < target && passes < max_scan {
        passes += 1;

        // Obtain next PFN in inactive list via CLOCK.
        let pfn = {
            let mut inactive = INACTIVE_LIST.lock();
            inactive.clock_next()
        };

        let pfn = match pfn {
            Some(p) => p,
            None => break, // list empty
        };

        // PINNED / DMA frame -> skip.
        if !frame_is_reclaimable(pfn) {
            RECLAIM_STATS.skipped.fetch_add(1, Ordering::Relaxed);
            result.skipped += 1;
            continue;
        }

        // Recently accessed frame -> promote to active.
        if frame_is_accessed(pfn) {
            frame_clear_accessed(pfn);
            let frame = frame_from_pfn(pfn);
            promote_to_active(frame);
            result.promoted += 1;
            RECLAIM_STATS.promoted.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        // Frame is candidate for free or swap.
        let frame = frame_from_pfn(pfn);
        let frame_flags = FRAME_DESCRIPTORS.get(frame).flags();

        if frame_flags.contains(FrameFlags::DIRTY) {
            // Write to swap before freeing.
            let swap_written = unsafe { try_swap_out(frame) };
            if swap_written {
                // Remove from inactive list and free physical frame.
                INACTIVE_LIST.lock().remove(pfn);
                let _ = free_page(frame);
                result.freed += 1;
                result.swapped += 1;
                RECLAIM_STATS.swapped_out.fetch_add(1, Ordering::Relaxed);
                RECLAIM_STATS.freed_direct.fetch_add(1, Ordering::Relaxed);
            } else {
                RECLAIM_STATS.swap_errors.fetch_add(1, Ordering::Relaxed);
                result.skipped += 1;
            }
        } else {
            // Clean frame — direct free.
            INACTIVE_LIST.lock().remove(pfn);
            let _ = free_page(frame);
            result.freed += 1;
            RECLAIM_STATS.freed_direct.fetch_add(1, Ordering::Relaxed);
        }

        // Handle active list overflow.
        if ACTIVE_LIST.lock().len() > HIGH_WATER_ACTIVE {
            let victim = ACTIVE_LIST.lock().pop();
            if let Some(v) = victim {
                let _ = INACTIVE_LIST.lock().push(v);
                RECLAIM_STATS.demoted.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    if cpu_id < RECLAIM_MAX_CPUS {
        leave_memalloc(cpu_id);
    }

    result
}

/// Attempts to write frame to swap.
/// Returns `true` if write succeeded.
///
/// # Safety
/// Accesses physmap — requires PHYS_MAP_BASE to be initialized.
unsafe fn try_swap_out(frame: Frame) -> bool {
    let (dev_idx, slot) = match SWAP_BACKEND.alloc_slot() {
        Ok(pair) => pair,
        Err(_) => return false,
    };
    // Write PAGE_SIZE bytes from physmap to swap device.
    match SWAP_BACKEND.write_page(dev_idx, slot, frame.phys_addr()) {
        Ok(_) => true,
        Err(_) => {
            // Free slot if write fails.
            SWAP_BACKEND.free_slot(dev_idx, slot);
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QUERY STATS
// ─────────────────────────────────────────────────────────────────────────────

/// Returns number of frames in each list.
pub fn lru_counts() -> (usize, usize) {
    let active = ACTIVE_LIST.lock().len();
    let inactive = INACTIVE_LIST.lock().len();
    (active, inactive)
}
