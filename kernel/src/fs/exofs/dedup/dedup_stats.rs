//! DedupStats — métriques de déduplication ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static DEDUP_STATS: DedupStats = DedupStats::new_const();

pub struct DedupStats {
    pub blobs_checked:    AtomicU64,
    pub blobs_deduped:    AtomicU64,
    pub bytes_saved:      AtomicU64,
    pub chunks_produced:  AtomicU64,
    pub chunks_matched:   AtomicU64,
    pub chunks_new:       AtomicU64,
    pub similarity_hits:  AtomicU64,
}

impl DedupStats {
    pub const fn new_const() -> Self {
        Self {
            blobs_checked:   AtomicU64::new(0),
            blobs_deduped:   AtomicU64::new(0),
            bytes_saved:     AtomicU64::new(0),
            chunks_produced: AtomicU64::new(0),
            chunks_matched:  AtomicU64::new(0),
            chunks_new:      AtomicU64::new(0),
            similarity_hits: AtomicU64::new(0),
        }
    }

    pub fn record_check(&self, data_size: u64) {
        self.blobs_checked.fetch_add(1, Ordering::Relaxed);
        let _ = data_size;
    }

    pub fn record_dedup(&self, saved_bytes: u64) {
        self.blobs_deduped.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved.fetch_add(saved_bytes, Ordering::Relaxed);
    }

    pub fn record_chunk(&self, matched: bool) {
        self.chunks_produced.fetch_add(1, Ordering::Relaxed);
        if matched {
            self.chunks_matched.fetch_add(1, Ordering::Relaxed);
        } else {
            self.chunks_new.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dedup_ratio_percent(&self) -> u64 {
        let total = self.blobs_checked.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        self.blobs_deduped.load(Ordering::Relaxed) * 100 / total
    }

    pub fn chunk_match_ratio_percent(&self) -> u64 {
        let total = self.chunks_produced.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        self.chunks_matched.load(Ordering::Relaxed) * 100 / total
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Snapshot {
        pub blobs_checked:   u64,
        pub blobs_deduped:   u64,
        pub bytes_saved:     u64,
        pub chunks_produced: u64,
        pub chunks_matched:  u64,
    }

    pub fn snapshot(&self) -> DedupStatsSnapshot {
        DedupStatsSnapshot {
            blobs_checked:   self.blobs_checked.load(Ordering::Relaxed),
            blobs_deduped:   self.blobs_deduped.load(Ordering::Relaxed),
            bytes_saved:     self.bytes_saved.load(Ordering::Relaxed),
            chunks_produced: self.chunks_produced.load(Ordering::Relaxed),
            chunks_matched:  self.chunks_matched.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DedupStatsSnapshot {
    pub blobs_checked:   u64,
    pub blobs_deduped:   u64,
    pub bytes_saved:     u64,
    pub chunks_produced: u64,
    pub chunks_matched:  u64,
}
