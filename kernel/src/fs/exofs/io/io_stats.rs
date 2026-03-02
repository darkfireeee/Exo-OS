//! Statistiques IO ExoFS — compteurs atomiques de latence et débit.

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs IO globaux.
pub struct IoStats {
    pub reads_total: AtomicU64,
    pub writes_total: AtomicU64,
    pub read_bytes_total: AtomicU64,
    pub write_bytes_total: AtomicU64,
    pub read_errors: AtomicU64,
    pub write_errors: AtomicU64,
    pub read_latency_ticks_sum: AtomicU64,
    pub write_latency_ticks_sum: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
}

impl IoStats {
    pub const fn new() -> Self {
        Self {
            reads_total: AtomicU64::new(0),
            writes_total: AtomicU64::new(0),
            read_bytes_total: AtomicU64::new(0),
            write_bytes_total: AtomicU64::new(0),
            read_errors: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            read_latency_ticks_sum: AtomicU64::new(0),
            write_latency_ticks_sum: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }

    pub fn record_read(&self, bytes: u64, latency_ticks: u64, ok: bool) {
        self.reads_total.fetch_add(1, Ordering::Relaxed);
        self.read_bytes_total.fetch_add(bytes, Ordering::Relaxed);
        self.read_latency_ticks_sum.fetch_add(latency_ticks, Ordering::Relaxed);
        if !ok {
            self.read_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_write(&self, bytes: u64, latency_ticks: u64, ok: bool) {
        self.writes_total.fetch_add(1, Ordering::Relaxed);
        self.write_bytes_total.fetch_add(bytes, Ordering::Relaxed);
        self.write_latency_ticks_sum.fetch_add(latency_ticks, Ordering::Relaxed);
        if !ok {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Débit moyen en bytes/tick (0 si aucune op).
    pub fn avg_read_bytes_per_tick(&self) -> u64 {
        let reads = self.reads_total.load(Ordering::Relaxed);
        if reads == 0 {
            return 0;
        }
        let bytes = self.read_bytes_total.load(Ordering::Relaxed);
        let ticks = self.read_latency_ticks_sum.load(Ordering::Relaxed);
        if ticks == 0 { bytes } else { bytes / ticks }
    }
}

/// Statistiques IO globales.
pub static IO_STATS: IoStats = IoStats::new();
