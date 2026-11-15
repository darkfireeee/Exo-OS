//! cache_warming.rs — ExoFS cache pre-warming at startup (no_std).
//!
//! `CacheWarmer`: priority queue for blob pre-warming.
//! `WarmingStrategy`: prefetch policy.
//! Rules: RECUR-01, OOM-02, ARITH-02.

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// WarmingStrategy
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-warming strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmingStrategy {
    /// Loads blobs in sequential queue order.
    Sequential,
    /// Prioritizes blobs accessed most frequently.
    ByFrequency,
    /// Prioritizes blobs accessed most recently.
    ByRecency,
    /// Selective: only blobs with score >= threshold.
    Selective { min_score: u32 },
}

// ─────────────────────────────────────────────────────────────────────────────
// WarmEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entry in the warming queue.
#[derive(Clone, Debug)]
pub struct WarmEntry {
    /// Blob identifier.
    pub blob_id: BlobId,
    /// Combined score (frequency * 1000 + relative_recency).
    pub score: u32,
    /// Estimated size in bytes.
    pub size_hint: u64,
    /// Last access ticks.
    pub last_ticks: u64,
    /// Historical access count.
    pub freq: u32,
}

impl WarmEntry {
    pub fn new(blob_id: BlobId, freq: u32, last_ticks: u64, size_hint: u64) -> Self {
        let score = freq.saturating_mul(1000).saturating_add(
            // normalizes recency (recent ticks = higher score)
            if last_ticks > 0 {
                (last_ticks % 1000) as u32
            } else {
                0
            },
        );
        Self {
            blob_id,
            score,
            size_hint,
            last_ticks,
            freq,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheWarmer
// ─────────────────────────────────────────────────────────────────────────────

/// Blob pre-warming queue with orderable strategy.
pub struct CacheWarmer {
    queue: Vec<WarmEntry>,
    strategy: WarmingStrategy,
    warmed: u64,
    skipped: u64,
    /// Maximum queue size.
    max_queue: usize,
}

impl CacheWarmer {
    pub fn new(strategy: WarmingStrategy, max_queue: usize) -> Self {
        Self {
            queue: Vec::new(),
            strategy,
            warmed: 0,
            skipped: 0,
            max_queue: max_queue.max(1),
        }
    }

    // ── Queue ────────────────────────────────────────────────────────────────

    /// Adds a blob to the warming queue.
    pub fn enqueue(&mut self, entry: WarmEntry) -> ExofsResult<()> {
        if self.queue.len() >= self.max_queue {
            return Err(ExofsError::NoSpace);
        }
        self.queue
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.queue.push(entry);
        Ok(())
    }

    /// Sorts the queue according to strategy.
    pub fn sort_queue(&mut self) {
        match self.strategy {
            WarmingStrategy::Sequential => { /* natural order, do nothing */ }
            WarmingStrategy::ByFrequency => {
                self.queue.sort_unstable_by(|a, b| b.freq.cmp(&a.freq));
            }
            WarmingStrategy::ByRecency => {
                self.queue
                    .sort_unstable_by(|a, b| b.last_ticks.cmp(&a.last_ticks));
            }
            WarmingStrategy::Selective { min_score } => {
                self.queue.retain(|e| e.score >= min_score);
                self.queue.sort_unstable_by(|a, b| b.score.cmp(&a.score));
            }
        }
    }

    /// Removes next `n` blobs to load.
    pub fn next_batch(&mut self, n: usize) -> Vec<WarmEntry> {
        let take = n.min(self.queue.len());
        // Iterative: drain first n elements.
        let mut batch = Vec::with_capacity(take);
        let remaining = self.queue.split_off(take);
        batch.extend(self.queue.drain(..));
        self.queue = remaining;
        self.warmed = self.warmed.wrapping_add(batch.len() as u64);
        batch
    }

    // ── Statistics ────────────────────────────────────────────────────────────

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
    pub fn warmed(&self) -> u64 {
        self.warmed
    }
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
    pub fn strategy(&self) -> WarmingStrategy {
        self.strategy
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Clears the queue.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Estimated total size of queued blobs.
    pub fn queued_bytes(&self) -> u64 {
        self.queue
            .iter()
            .map(|e| e.size_hint)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Removes blobs already present in the cache (membership predicate).
    pub fn dedup_with<F>(&mut self, already_cached: F)
    where
        F: Fn(&BlobId) -> bool,
    {
        let before = self.queue.len();
        self.queue.retain(|e| !already_cached(&e.blob_id));
        let removed = before.saturating_sub(self.queue.len());
        self.skipped = self.skipped.wrapping_add(removed as u64);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }
    fn entry(b: u8, freq: u32, ticks: u64) -> WarmEntry {
        WarmEntry::new(blob(b), freq, ticks, 512)
    }

    #[test]
    fn test_enqueue_and_len() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 5, 100)).test_unwrap();
        w.enqueue(entry(2, 3, 200)).test_unwrap();
        assert_eq!(w.queue_len(), 2);
    }

    #[test]
    fn test_enqueue_overflow() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 2);
        w.enqueue(entry(1, 1, 1)).test_unwrap();
        w.enqueue(entry(2, 1, 1)).test_unwrap();
        assert!(w.enqueue(entry(3, 1, 1)).is_err());
    }

    #[test]
    fn test_sort_by_frequency() {
        let mut w = CacheWarmer::new(WarmingStrategy::ByFrequency, 10);
        w.enqueue(entry(1, 1, 0)).test_unwrap();
        w.enqueue(entry(2, 9, 0)).test_unwrap();
        w.sort_queue();
        let batch = w.next_batch(1);
        assert_eq!(batch[0].blob_id, blob(2));
    }

    #[test]
    fn test_sort_by_recency() {
        let mut w = CacheWarmer::new(WarmingStrategy::ByRecency, 10);
        w.enqueue(entry(1, 1, 50)).test_unwrap();
        w.enqueue(entry(2, 1, 200)).test_unwrap();
        w.sort_queue();
        let batch = w.next_batch(1);
        assert_eq!(batch[0].blob_id, blob(2));
    }

    #[test]
    fn test_selective_filters() {
        let mut w = CacheWarmer::new(WarmingStrategy::Selective { min_score: 5000 }, 10);
        w.enqueue(entry(1, 1, 0)).test_unwrap(); // score = 1000
        w.enqueue(entry(2, 6, 0)).test_unwrap(); // score = 6000
        w.sort_queue();
        assert_eq!(w.queue_len(), 1);
        assert_eq!(w.queue[0].blob_id, blob(2));
    }

    #[test]
    fn test_next_batch_reduces_queue() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        for i in 0..5u8 {
            w.enqueue(entry(i, 1, 0)).test_unwrap();
        }
        let b = w.next_batch(3);
        assert_eq!(b.len(), 3);
        assert_eq!(w.queue_len(), 2);
    }

    #[test]
    fn test_warmed_counter() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).test_unwrap();
        w.enqueue(entry(2, 1, 0)).test_unwrap();
        w.next_batch(2);
        assert_eq!(w.warmed(), 2);
    }

    #[test]
    fn test_queued_bytes() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(WarmEntry::new(blob(1), 1, 0, 1024)).test_unwrap();
        assert_eq!(w.queued_bytes(), 1024);
    }

    #[test]
    fn test_dedup_with() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).test_unwrap();
        w.enqueue(entry(2, 1, 0)).test_unwrap();
        w.dedup_with(|b| *b == blob(1));
        assert_eq!(w.queue_len(), 1);
        assert_eq!(w.skipped(), 1);
    }

    #[test]
    fn test_clear() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).test_unwrap();
        w.clear();
        assert!(w.is_empty());
    }
}

// ── CacheWarmer extensions ───────────────────────────────────────────────────

impl CacheWarmer {
    /// Inserts a batch of entries without failing on overflow — returns count inserted.
    pub fn enqueue_batch_best_effort(&mut self, entries: &[WarmEntry]) -> usize {
        let mut count = 0usize;
        for e in entries {
            if self.enqueue(e.clone()).is_ok() {
                count = count.wrapping_add(1);
            }
        }
        count
    }

    /// Inspects the first element without consuming it.
    pub fn peek(&self) -> Option<&WarmEntry> {
        self.queue.first()
    }

    /// Drains and returns all elements.
    pub fn drain_all(&mut self) -> Vec<WarmEntry> {
        let all: Vec<WarmEntry> = self.queue.drain(..).collect();
        self.warmed = self.warmed.wrapping_add(all.len() as u64);
        all
    }

    /// Returns high-scoring blobs without removing them from queue.
    pub fn high_priority(&self, min_score: u32) -> Vec<&WarmEntry> {
        self.queue.iter().filter(|e| e.score >= min_score).collect()
    }

    /// Prunes entries whose score is below `min_score`.
    pub fn prune_below(&mut self, min_score: u32) -> usize {
        let before = self.queue.len();
        self.queue.retain(|e| e.score >= min_score);
        let removed = before.saturating_sub(self.queue.len());
        self.skipped = self.skipped.wrapping_add(removed as u64);
        removed
    }

    /// Number of queued blobs with size_hint > `threshold`.
    pub fn count_large(&self, threshold: u64) -> usize {
        self.queue
            .iter()
            .filter(|e| e.size_hint > threshold)
            .count()
    }
}
