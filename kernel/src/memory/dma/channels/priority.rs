// kernel/src/memory/dma/channels/priority.rs
//
// Priority scheduling for DMA channels — RT vs best-effort queues.
//
// Architecture:
//   - 4 priority levels: Low, Normal, High, Realtime (from DmaPriority).
//   - A dedicated circular queue per level (CHANNEL_QUEUE_DEPTH entries/level).
//   - The DMA scheduler always dequeues in decreasing order of priority.
//   - RT transactions have a deadline (time-to-live in scheduler passes).
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::dma::core::types::{DmaChannelId, DmaError, DmaPriority, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Number of priority levels (Low=0, Normal=1, High=2, Realtime=3).
pub const PRIORITY_LEVELS: usize = 4;

/// Depth of each priority queue (in transactions).
pub const PRIORITY_QUEUE_DEPTH: usize = 64;

/// Number of passes before a best-effort transaction is promoted
/// to prevent starvation.
pub const STARVATION_THRESHOLD: u32 = 32;

// ─────────────────────────────────────────────────────────────────────────────
// PRIORITY QUEUE ENTRY
// ─────────────────────────────────────────────────────────────────────────────

/// Entry in a priority queue.
#[derive(Copy, Clone)]
struct PrioEntry {
    /// Associated DMA transaction.
    txn_id: DmaTransactionId,
    /// Requesting DMA channel.
    channel_id: DmaChannelId,
    /// Number of scheduler passes elapsed without being dequeued (starvation prevention).
    age_passes: u32,
}

impl PrioEntry {
    const EMPTY: Self = PrioEntry {
        txn_id: DmaTransactionId::INVALID,
        channel_id: DmaChannelId(u32::MAX),
        age_passes: 0,
    };

    #[allow(dead_code)]
    fn is_valid(self) -> bool {
        self.txn_id.0 != DmaTransactionId::INVALID.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CIRCULAR QUEUE PER LEVEL
// ─────────────────────────────────────────────────────────────────────────────

struct PrioQueue {
    entries: [PrioEntry; PRIORITY_QUEUE_DEPTH],
    head: usize,
    tail: usize,
    count: usize,
}

impl PrioQueue {
    const fn new() -> Self {
        PrioQueue {
            entries: [PrioEntry::EMPTY; PRIORITY_QUEUE_DEPTH],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, entry: PrioEntry) -> bool {
        if self.count >= PRIORITY_QUEUE_DEPTH {
            return false;
        }
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % PRIORITY_QUEUE_DEPTH;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<PrioEntry> {
        if self.count == 0 {
            return None;
        }
        let entry = self.entries[self.head];
        self.entries[self.head] = PrioEntry::EMPTY;
        self.head = (self.head + 1) % PRIORITY_QUEUE_DEPTH;
        self.count -= 1;
        Some(entry)
    }

    fn peek(&self) -> Option<&PrioEntry> {
        if self.count == 0 {
            return None;
        }
        Some(&self.entries[self.head])
    }

    fn len(&self) -> usize {
        self.count
    }
    #[allow(dead_code)]
    fn is_full(&self) -> bool {
        self.count >= PRIORITY_QUEUE_DEPTH
    }
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Increments the age of all entries (starvation prevention).
    fn age_all(&mut self) {
        let mut idx = self.head;
        for _ in 0..self.count {
            self.entries[idx].age_passes = self.entries[idx].age_passes.saturating_add(1);
            idx = (idx + 1) % PRIORITY_QUEUE_DEPTH;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MULTI-PRIORITY SCHEDULER
// ─────────────────────────────────────────────────────────────────────────────

/// Priority scheduler statistics.
pub struct PrioritySchedulerStats {
    pub enqueued: [AtomicU64; PRIORITY_LEVELS],
    pub dequeued: [AtomicU64; PRIORITY_LEVELS],
    pub promoted: AtomicU64,
    pub full_drops: AtomicU64,
}

impl Default for PrioritySchedulerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PrioritySchedulerStats {
    const fn new() -> Self {
        PrioritySchedulerStats {
            enqueued: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            dequeued: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            promoted: AtomicU64::new(0),
            full_drops: AtomicU64::new(0),
        }
    }
}

struct PrioritySchedulerInner {
    queues: [PrioQueue; PRIORITY_LEVELS],
    /// Number of passes since the last starvation prevention sweep.
    pass_counter: u32,
}

impl PrioritySchedulerInner {
    const fn new() -> Self {
        PrioritySchedulerInner {
            queues: [
                PrioQueue::new(),
                PrioQueue::new(),
                PrioQueue::new(),
                PrioQueue::new(),
            ],
            pass_counter: 0,
        }
    }

    fn prio_index(p: DmaPriority) -> usize {
        match p {
            DmaPriority::Low => 0,
            DmaPriority::Normal => 1,
            DmaPriority::High => 2,
            DmaPriority::Realtime => 3,
        }
    }
}

/// Multi-priority scheduler for DMA transactions.
///
/// Thread-safe via spinlock. All operations are O(1) except
/// the starvation prevention sweep which is O(PRIORITY_QUEUE_DEPTH * PRIORITY_LEVELS).
pub struct PriorityScheduler {
    inner: Mutex<PrioritySchedulerInner>,
    pub stats: PrioritySchedulerStats,
}

// SAFETY: PriorityScheduler is protected by a Mutex.
unsafe impl Sync for PriorityScheduler {}
unsafe impl Send for PriorityScheduler {}

impl Default for PriorityScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PriorityScheduler {
    pub const fn new() -> Self {
        PriorityScheduler {
            inner: Mutex::new(PrioritySchedulerInner::new()),
            stats: PrioritySchedulerStats::new(),
        }
    }

    /// Enqueues a transaction with the given priority.
    ///
    /// Returns `Err(DmaError::OutOfMemory)` if the queue for this level is full.
    pub fn enqueue(
        &self,
        txn_id: DmaTransactionId,
        channel_id: DmaChannelId,
        priority: DmaPriority,
    ) -> Result<(), DmaError> {
        let level = PrioritySchedulerInner::prio_index(priority);
        let entry = PrioEntry {
            txn_id,
            channel_id,
            age_passes: 0,
        };

        let mut inner = self.inner.lock();
        if !inner.queues[level].push(entry) {
            self.stats.full_drops.fetch_add(1, Ordering::Relaxed);
            return Err(DmaError::OutOfMemory);
        }
        self.stats.enqueued[level].fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Dequeues the next transaction according to highest priority.
    ///
    /// First checks aged entries (starvation prevention) for lower levels,
    /// then dequeues from the highest non-empty priority queue.
    ///
    /// Returns `None` if no transaction is pending.
    pub fn dequeue(&self) -> Option<(DmaTransactionId, DmaChannelId, DmaPriority)> {
        let mut inner = self.inner.lock();

        // Starvation prevention: every N passes, promote old entries.
        inner.pass_counter = inner.pass_counter.wrapping_add(1);
        if inner.pass_counter.is_multiple_of(STARVATION_THRESHOLD) {
            self.check_starvation(&mut inner);
        }

        // Dequeue in descending priority order (3 = Realtime -> 0 = Low).
        for level in (0..PRIORITY_LEVELS).rev() {
            if let Some(entry) = inner.queues[level].pop() {
                let prio = match level {
                    3 => DmaPriority::Realtime,
                    2 => DmaPriority::High,
                    1 => DmaPriority::Normal,
                    _ => DmaPriority::Low,
                };
                self.stats.dequeued[level].fetch_add(1, Ordering::Relaxed);
                return Some((entry.txn_id, entry.channel_id, prio));
            }
        }
        None
    }

    /// Returns the total number of pending transactions.
    pub fn total_pending(&self) -> usize {
        let inner = self.inner.lock();
        inner.queues.iter().map(|q| q.len()).sum()
    }

    /// Returns the number of pending transactions per level.
    pub fn pending_by_level(&self) -> [usize; PRIORITY_LEVELS] {
        let inner = self.inner.lock();
        [
            inner.queues[0].len(),
            inner.queues[1].len(),
            inner.queues[2].len(),
            inner.queues[3].len(),
        ]
    }

    // ── Starvation Prevention ───────────────────────────────────────────────

    /// Checks if low-priority entries are too old and promotes them.
    fn check_starvation(&self, inner: &mut PrioritySchedulerInner) {
        // For each level < Realtime, look for aged entries.
        for src_level in 0..PRIORITY_LEVELS - 1 {
            let dst_level = (src_level + 1).min(PRIORITY_LEVELS - 1);
            // Age all entries of the source level.
            inner.queues[src_level].age_all();

            // Promote entries that exceeded the threshold.
            let src_len = inner.queues[src_level].len();
            let mut promoted_count = 0;
            for _ in 0..src_len {
                let aged = match inner.queues[src_level].peek() {
                    Some(e) => e.age_passes >= STARVATION_THRESHOLD,
                    None => false,
                };
                if aged {
                    if let Some(mut entry) = inner.queues[src_level].pop() {
                        entry.age_passes = 0;
                        if inner.queues[dst_level].push(entry) {
                            promoted_count += 1;
                        } else {
                            // Destination queue full — put back into source.
                            inner.queues[src_level].push(entry);
                        }
                    }
                } else {
                    break; // Remaining entries not aged, FIFO preserved.
                }
            }
            if promoted_count > 0 {
                self.stats
                    .promoted
                    .fetch_add(promoted_count as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Global instance of the DMA priority scheduler.
pub static DMA_PRIORITY_SCHEDULER: PriorityScheduler = PriorityScheduler::new();
