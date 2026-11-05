// kernel/src/memory/cow/tracker.rs
//
// CoW tracker (Copy-on-Write) — maintains reference counting
// for frames shared between processes (fork/mmap shared).
// LAYER 0 — no external dependencies.

use crate::memory::core::types::Frame;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// CoW COUNT TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// Number of simultaneously trackable frames (power of 2 for hash).
///
/// Userspace binaries and their servers can share far more than 4096
/// pages after a few fork/execs. Saturating here must remain exceptional,
/// not become a practical terminal limit.
pub const COW_TABLE_SIZE: usize = 65536;
/// Hash mask.
pub const COW_TABLE_MASK: usize = COW_TABLE_SIZE - 1;

/// Sentinel: slot never used.
const SLOT_EMPTY: u64 = 0;
/// Sentinel: slot deleted (tombstone). Prevents breaking the linear
/// probing chain upon deletion.
const SLOT_DELETED: u64 = u64::MAX;

/// A CoW entry in the hash table.
#[repr(C, align(16))]
struct CowEntry {
    /// Frame number (0 = free slot, u64::MAX = deleted tombstone).
    frame_idx: AtomicU64,
    /// Number of mappings for this frame (CoW refcount).
    ref_count: AtomicU32,
    _pad: u32,
}

impl CowEntry {
    #[allow(dead_code)]
    const fn new() -> Self {
        CowEntry {
            frame_idx: AtomicU64::new(0),
            ref_count: AtomicU32::new(0),
            _pad: 0,
        }
    }
}

/// CoW hash table with linear probing.
/// Stores shared frames (refcount >= 2).
pub struct CowTracker {
    table: [CowEntry; COW_TABLE_SIZE],
    /// Global lock protecting inc/dec against TOCTOU races.
    lock: Mutex<()>,
    pub tracked_count: AtomicU64,
    pub inc_count: AtomicU64,
    pub dec_count: AtomicU64,
    pub overflow_count: AtomicU64,
    pub collision_max: AtomicU32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowTrackerError {
    TableFull,
}

// SAFETY: CowTracker uses an internal Mutex for all mutations.
unsafe impl Sync for CowTracker {}
unsafe impl Send for CowTracker {}

impl Default for CowTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CowTracker {
    const fn new() -> Self {
        CowTracker {
            table: [const { CowEntry::new() }; COW_TABLE_SIZE],
            lock: Mutex::new(()),
            tracked_count: AtomicU64::new(0),
            inc_count: AtomicU64::new(0),
            dec_count: AtomicU64::new(0),
            overflow_count: AtomicU64::new(0),
            collision_max: AtomicU32::new(0),
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn log_overflow(count: u64) {
        if count == 1 || count.is_power_of_two() {
            crate::arch::x86_64::terminal::debug_write(b"[COW:OVF count=");
            let mut buf = [0u8; 20];
            let mut value = count;
            let mut pos = buf.len();
            if value == 0 {
                pos -= 1;
                buf[pos] = b'0';
            } else {
                while value > 0 && pos > 0 {
                    pos -= 1;
                    buf[pos] = b'0' + (value % 10) as u8;
                    value /= 10;
                }
            }
            crate::arch::x86_64::terminal::debug_write(&buf[pos..]);
            crate::arch::x86_64::terminal::debug_write(b"]\n");
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn log_overflow(_count: u64) {}

    /// FNV-1a frame number hash (fast, no division).
    #[inline]
    fn hash(frame_idx: u64) -> usize {
        let mut h = 0xcbf29ce484222325u64;
        h ^= frame_idx;
        h = h.wrapping_mul(0x00000100000001B3);
        (h as usize) & COW_TABLE_MASK
    }

    /// Increments the CoW refcount of a frame.
    /// If the frame is not in the table, inserts it with refcount=2.
    pub fn try_inc(&self, frame: Frame) -> Result<u32, CowTrackerError> {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        let _guard = self.lock.lock();
        let mut first_tombstone: Option<usize> = None;

        for (collisions, probe) in (0..COW_TABLE_SIZE).enumerate() {
            let collisions = collisions as u32;
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Relaxed);

            if existing == idx {
                // Slot found — increment under lock.
                let rc = entry.ref_count.fetch_add(1, Ordering::Relaxed) + 1;
                self.inc_count.fetch_add(1, Ordering::Relaxed);
                return Ok(rc);
            }
            if existing == SLOT_EMPTY {
                // End of chain — insert into first available tombstone
                // or into this empty slot.
                let insert_slot = first_tombstone.unwrap_or(slot);
                let entry_ins = &self.table[insert_slot];
                entry_ins.frame_idx.store(idx, Ordering::Relaxed);
                entry_ins.ref_count.store(2, Ordering::Relaxed);
                self.tracked_count.fetch_add(1, Ordering::Relaxed);
                self.inc_count.fetch_add(1, Ordering::Relaxed);
                let old_max = self.collision_max.load(Ordering::Relaxed);
                if collisions > old_max {
                    self.collision_max.store(collisions, Ordering::Relaxed);
                }
                return Ok(2);
            }
            if existing == SLOT_DELETED && first_tombstone.is_none() {
                first_tombstone = Some(slot);
            }
        }
        // Table full — degenerate case.
        let overflow = self.overflow_count.fetch_add(1, Ordering::Relaxed) + 1;
        Self::log_overflow(overflow);
        Err(CowTrackerError::TableFull)
    }

    /// Increments the CoW refcount of a frame.
    ///
    /// Historical API kept for code paths that cannot propagate
    /// errors. New critical paths must use `try_inc()`.
    pub fn inc(&self, frame: Frame) -> u32 {
        self.try_inc(frame).unwrap_or(u32::MAX)
    }

    /// Decrements the CoW refcount of a frame.
    /// Returns the new refcount (0 = frame can be freed/reclaimed for write).
    pub fn dec(&self, frame: Frame) -> u32 {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        let _guard = self.lock.lock();

        for probe in 0..COW_TABLE_SIZE {
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Relaxed);

            if existing == SLOT_EMPTY {
                break; // Real end of chain — frame not tracked
            }
            if existing == SLOT_DELETED {
                continue; // Tombstone — continue probing
            }
            if existing == idx {
                let old = entry.ref_count.load(Ordering::Relaxed);
                if old <= 1 {
                    // Last reference — mark as tombstone (does not break chain).
                    entry.frame_idx.store(SLOT_DELETED, Ordering::Relaxed);
                    entry.ref_count.store(0, Ordering::Relaxed);
                    self.tracked_count.fetch_sub(1, Ordering::Relaxed);
                    self.dec_count.fetch_add(1, Ordering::Relaxed);
                    return 0;
                }
                let new_rc = entry.ref_count.fetch_sub(1, Ordering::Relaxed) - 1;
                if new_rc == 0 {
                    // Transition to zero: tombstone (concurrent case under lock).
                    entry.frame_idx.store(SLOT_DELETED, Ordering::Relaxed);
                    self.tracked_count.fetch_sub(1, Ordering::Relaxed);
                }
                self.dec_count.fetch_add(1, Ordering::Relaxed);
                return new_rc;
            }
        }
        u32::MAX // Not found -> explicit sentinel, do not free frame under any circumstances
    }

    /// Returns the current refcount without modification.
    pub fn ref_count(&self, frame: Frame) -> u32 {
        self.tracked_ref_count(frame).unwrap_or(1)
    }

    /// Returns the refcount only if the frame is explicitly tracked.
    pub fn tracked_ref_count(&self, frame: Frame) -> Option<u32> {
        let idx = frame.phys_addr().as_u64() / 4096;
        let start = Self::hash(idx);
        let _guard = self.lock.lock();
        for probe in 0..COW_TABLE_SIZE {
            let slot = (start + probe) & COW_TABLE_MASK;
            let entry = &self.table[slot];
            let existing = entry.frame_idx.load(Ordering::Acquire);
            if existing == SLOT_EMPTY {
                return None;
            }
            if existing == SLOT_DELETED {
                continue;
            }
            if existing == idx {
                return Some(entry.ref_count.load(Ordering::Acquire));
            }
        }
        None
    }

    /// Indicates whether a frame is shared (refcount >= 2).
    #[inline]
    pub fn is_shared(&self, frame: Frame) -> bool {
        self.ref_count(frame) >= 2
    }

    #[inline]
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }
}

pub static COW_TRACKER: CowTracker = CowTracker::new();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::core::PhysAddr;

    fn frame_at(addr: u64) -> Frame {
        Frame::containing(PhysAddr::new(addr))
    }

    #[test]
    fn dec_on_untracked_frame_returns_sentinel() {
        let tracker = CowTracker::new();
        assert_eq!(tracker.dec(frame_at(0x4000)), u32::MAX);
    }

    #[test]
    fn ref_count_reads_consistent_tracked_value() {
        let tracker = CowTracker::new();
        let frame = frame_at(0x8000);
        assert_eq!(tracker.inc(frame), 2);
        assert_eq!(tracker.ref_count(frame), 2);
    }
}
