//! metrics.rs — Métriques globales ExoFS (no_std).
//!
//! Fournit :
//!  - `MetricId`          : identifiant de métrique (enum).
//!  - `MetricKind`        : type (counter / gauge / ratio).
//!  - `ExofsMetrics`      : compteurs atomiques globaux.
//!  - `MetricsSnapshot`   : snapshot immutable.
//!  - `MetricsHistory`    : ring de snapshots horodatés.
//!  - `MetricsDiff`       : différence entre deux snapshots.
//!  - `EXOFS_METRICS`     : singleton global.
//!
//! RECUR-01 : while uniquement.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── MetricId ────────────────────────────────────────────────────────────────

/// Identifiant structuré d'une métrique.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricId {
    Reads         = 0,
    Writes        = 1,
    ReadBytes     = 2,
    WriteBytes    = 3,
    CacheHits     = 4,
    CacheMisses   = 5,
    Errors        = 6,
    GcRuns        = 7,
    DedupSaves    = 8,
    EpochCommits  = 9,
    Allocations   = 10,
    Deallocations = 11,
    FlushOps      = 12,
    CoWCopies     = 13,
    SnapshotCreates = 14,
    SnapshotDeletes = 15,
}

impl MetricId {
    pub const COUNT: usize = 16;

    pub fn name(self) -> &'static str {
        match self {
            Self::Reads           => "reads",
            Self::Writes          => "writes",
            Self::ReadBytes       => "read_bytes",
            Self::WriteBytes      => "write_bytes",
            Self::CacheHits       => "cache_hits",
            Self::CacheMisses     => "cache_misses",
            Self::Errors          => "errors",
            Self::GcRuns          => "gc_runs",
            Self::DedupSaves      => "dedup_saves",
            Self::EpochCommits    => "epoch_commits",
            Self::Allocations     => "allocations",
            Self::Deallocations   => "deallocations",
            Self::FlushOps        => "flush_ops",
            Self::CoWCopies       => "cow_copies",
            Self::SnapshotCreates => "snapshot_creates",
            Self::SnapshotDeletes => "snapshot_deletes",
        }
    }

    pub fn is_byte_counter(self) -> bool {
        matches!(self, Self::ReadBytes | Self::WriteBytes | Self::DedupSaves)
    }
}

// ─── MetricKind ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricKind {
    Counter,   // monotone croissant
    Gauge,     // valeur absolue instantanée
    Ratio,     // rapport calculé (pas stocké directement)
}

// ─── ExofsMetrics ────────────────────────────────────────────────────────────

/// Compteurs atomiques globaux du filesystem.
pub struct ExofsMetrics {
    counters: [AtomicU64; MetricId::COUNT],
}

// SAFETY : AtomicU64 est Sync+Send.
unsafe impl Sync for ExofsMetrics {}
unsafe impl Send for ExofsMetrics {}

impl ExofsMetrics {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self { counters: [ZERO; MetricId::COUNT] }
    }

    #[inline]
    fn add(&self, id: MetricId, n: u64) {
        self.counters[id as usize].fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self, id: MetricId) -> u64 {
        self.counters[id as usize].load(Ordering::Relaxed)
    }

    // ── Accesseurs sémantiques ───────────────────────────────────────────────

    pub fn inc_read(&self, bytes: u64) {
        self.add(MetricId::Reads, 1);
        self.add(MetricId::ReadBytes, bytes);
    }

    pub fn inc_write(&self, bytes: u64) {
        self.add(MetricId::Writes, 1);
        self.add(MetricId::WriteBytes, bytes);
    }

    pub fn inc_cache_hit(&self)  { self.add(MetricId::CacheHits, 1); }
    pub fn inc_cache_miss(&self) { self.add(MetricId::CacheMisses, 1); }
    pub fn inc_error(&self)      { self.add(MetricId::Errors, 1); }
    pub fn inc_gc(&self)         { self.add(MetricId::GcRuns, 1); }
    pub fn inc_dedup_save(&self, bytes: u64) { self.add(MetricId::DedupSaves, bytes); }
    pub fn inc_epoch_commit(&self)  { self.add(MetricId::EpochCommits, 1); }
    pub fn inc_alloc(&self)         { self.add(MetricId::Allocations, 1); }
    pub fn inc_dealloc(&self)       { self.add(MetricId::Deallocations, 1); }
    pub fn inc_flush(&self)         { self.add(MetricId::FlushOps, 1); }
    pub fn inc_cow(&self)           { self.add(MetricId::CoWCopies, 1); }
    pub fn inc_snapshot_create(&self) { self.add(MetricId::SnapshotCreates, 1); }
    pub fn inc_snapshot_delete(&self) { self.add(MetricId::SnapshotDeletes, 1); }

    /// Cache hit ratio * 1000 (ARITH-02: checked_div).
    pub fn cache_hit_ratio_pct10(&self) -> u64 {
        let hits   = self.get(MetricId::CacheHits);
        let misses = self.get(MetricId::CacheMisses);
        let total  = hits.saturating_add(misses);
        hits.saturating_mul(1000).checked_div(total).unwrap_or(0)
    }

    /// Taux d'erreur * 1000 (ARITH-02).
    pub fn error_rate_pct10(&self) -> u64 {
        let ops = self.get(MetricId::Reads).saturating_add(self.get(MetricId::Writes));
        let err = self.get(MetricId::Errors);
        err.saturating_mul(1000).checked_div(ops).unwrap_or(0)
    }

    /// Snapshot atomique des compteurs.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut values = [0u64; MetricId::COUNT];
        let mut i = 0usize;
        while i < MetricId::COUNT {
            values[i] = self.counters[i].load(Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
        MetricsSnapshot { values }
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        let mut i = 0usize;
        while i < MetricId::COUNT {
            self.counters[i].store(0, Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
    }
}

/// Singleton global.
pub static EXOFS_METRICS: ExofsMetrics = ExofsMetrics::new_const();

// ─── MetricsSnapshot ─────────────────────────────────────────────────────────

/// Snapshot immuable de tous les compteurs.
#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsSnapshot {
    pub values: [u64; MetricId::COUNT],
}

impl MetricsSnapshot {
    pub fn get(&self, id: MetricId) -> u64 { self.values[id as usize] }

    pub fn reads(&self)        -> u64 { self.get(MetricId::Reads) }
    pub fn writes(&self)       -> u64 { self.get(MetricId::Writes) }
    pub fn read_bytes(&self)   -> u64 { self.get(MetricId::ReadBytes) }
    pub fn write_bytes(&self)  -> u64 { self.get(MetricId::WriteBytes) }
    pub fn cache_hits(&self)   -> u64 { self.get(MetricId::CacheHits) }
    pub fn cache_misses(&self) -> u64 { self.get(MetricId::CacheMisses) }
    pub fn errors(&self)       -> u64 { self.get(MetricId::Errors) }
    pub fn gc_runs(&self)      -> u64 { self.get(MetricId::GcRuns) }
    pub fn epoch_commits(&self)-> u64 { self.get(MetricId::EpochCommits) }

    pub fn total_ops(&self) -> u64 {
        self.reads().saturating_add(self.writes())
    }

    pub fn total_bytes(&self) -> u64 {
        self.read_bytes().saturating_add(self.write_bytes())
    }

    /// Différence avec un snapshot précédent.
    pub fn diff(&self, prev: &MetricsSnapshot) -> MetricsDiff {
        let mut delta = [0u64; MetricId::COUNT];
        let mut i = 0usize;
        while i < MetricId::COUNT {
            delta[i] = self.values[i].saturating_sub(prev.values[i]);
            i = i.wrapping_add(1);
        }
        MetricsDiff { delta }
    }
}

// ─── MetricsDiff ─────────────────────────────────────────────────────────────

/// Écart entre deux snapshots.
#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsDiff {
    pub delta: [u64; MetricId::COUNT],
}

impl MetricsDiff {
    pub fn get(&self, id: MetricId) -> u64 { self.delta[id as usize] }
    pub fn read_rate(&self)  -> u64 { self.get(MetricId::Reads) }
    pub fn write_rate(&self) -> u64 { self.get(MetricId::Writes) }
    pub fn error_delta(&self)-> u64 { self.get(MetricId::Errors) }
    pub fn is_clean(&self)   -> bool { self.error_delta() == 0 }
}

// ─── MetricsHistory ──────────────────────────────────────────────────────────

pub const METRICS_HISTORY_SIZE: usize = 64;

/// Ring de snapshots horodatés.
pub struct MetricsHistory {
    slots: [MetricsHistorySlot; METRICS_HISTORY_SIZE],
    head:  core::sync::atomic::AtomicU64,
}

#[derive(Clone, Copy, Default)]
pub struct MetricsHistorySlot {
    pub tick:     u64,
    pub snapshot: MetricsSnapshot,
}

// SAFETY : accès par index tournant atomique.
unsafe impl Sync for MetricsHistory {}

impl MetricsHistory {
    pub const fn new_const() -> Self {
        const ZERO: MetricsHistorySlot = MetricsHistorySlot {
            tick: 0, snapshot: MetricsSnapshot { values: [0u64; MetricId::COUNT] },
        };
        Self { slots: [ZERO; METRICS_HISTORY_SIZE], head: core::sync::atomic::AtomicU64::new(0) }
    }

    /// Enregistre le snapshot courant (tick fourni en paramètre).
    pub fn record(&self, tick: u64, snap: MetricsSnapshot) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % METRICS_HISTORY_SIZE;
        // SAFETY : index tournant exclusif.
        unsafe {
            let ptr = &self.slots[idx] as *const MetricsHistorySlot as *mut MetricsHistorySlot;
            (*ptr).tick     = tick;
            (*ptr).snapshot = snap;
        }
    }

    /// Retourne le dernier slot enregistré.
    pub fn latest(&self) -> MetricsHistorySlot {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx  = (head.wrapping_add(METRICS_HISTORY_SIZE).wrapping_sub(1)) % METRICS_HISTORY_SIZE;
        self.slots[idx]
    }

    /// Itère les n derniers slots (RECUR-01 : while).
    pub fn last_n(&self, n: usize, out: &mut Vec<MetricsHistorySlot>) -> ExofsResult<()> {
        let n = n.min(METRICS_HISTORY_SIZE);
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n {
            let idx = (head.wrapping_add(METRICS_HISTORY_SIZE).wrapping_sub(i).wrapping_sub(1)) % METRICS_HISTORY_SIZE;
            out.push(self.slots[idx]);
            i = i.wrapping_add(1);
        }
        Ok(())
    }
}

pub static METRICS_HISTORY: MetricsHistory = MetricsHistory::new_const();

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inc_read() {
        let m = ExofsMetrics::new_const();
        m.inc_read(512);
        assert_eq!(m.get(MetricId::Reads), 1);
        assert_eq!(m.get(MetricId::ReadBytes), 512);
    }

    #[test]
    fn test_inc_write() {
        let m = ExofsMetrics::new_const();
        m.inc_write(1024);
        assert_eq!(m.get(MetricId::Writes), 1);
        assert_eq!(m.get(MetricId::WriteBytes), 1024);
    }

    #[test]
    fn test_cache_hit_ratio() {
        let m = ExofsMetrics::new_const();
        m.inc_cache_hit(); m.inc_cache_hit();
        m.inc_cache_miss();
        // 2/3 * 1000 = 666
        assert_eq!(m.cache_hit_ratio_pct10(), 666);
    }

    #[test]
    fn test_cache_hit_ratio_zero_ops() {
        let m = ExofsMetrics::new_const();
        assert_eq!(m.cache_hit_ratio_pct10(), 0);
    }

    #[test]
    fn test_snapshot_values() {
        let m = ExofsMetrics::new_const();
        m.inc_gc(); m.inc_gc();
        let snap = m.snapshot();
        assert_eq!(snap.gc_runs(), 2);
    }

    #[test]
    fn test_snapshot_diff() {
        let m = ExofsMetrics::new_const();
        let s0 = m.snapshot();
        m.inc_read(1); m.inc_read(1);
        let s1 = m.snapshot();
        let d = s1.diff(&s0);
        assert_eq!(d.read_rate(), 2);
    }

    #[test]
    fn test_diff_is_clean() {
        let s0 = MetricsSnapshot::default();
        let s1 = MetricsSnapshot::default();
        let d = s1.diff(&s0);
        assert!(d.is_clean());
    }

    #[test]
    fn test_total_ops_and_bytes() {
        let m = ExofsMetrics::new_const();
        m.inc_read(100); m.inc_write(200);
        let s = m.snapshot();
        assert_eq!(s.total_ops(), 2);
        assert_eq!(s.total_bytes(), 300);
    }

    #[test]
    fn test_reset() {
        let m = ExofsMetrics::new_const();
        m.inc_error(); m.inc_error();
        m.reset();
        assert_eq!(m.get(MetricId::Errors), 0);
    }

    #[test]
    fn test_metric_id_name() {
        assert_eq!(MetricId::Reads.name(), "reads");
        assert_eq!(MetricId::CacheHits.name(), "cache_hits");
    }

    #[test]
    fn test_metric_id_is_byte_counter() {
        assert!(MetricId::ReadBytes.is_byte_counter());
        assert!(!MetricId::Reads.is_byte_counter());
    }

    #[test]
    fn test_history_record_latest() {
        let h = MetricsHistory::new_const();
        let snap = MetricsSnapshot::default();
        h.record(42, snap);
        assert_eq!(h.latest().tick, 42);
    }

    #[test]
    fn test_history_last_n() {
        let h = MetricsHistory::new_const();
        h.record(1, MetricsSnapshot::default());
        h.record(2, MetricsSnapshot::default());
        let mut out = Vec::new();
        h.last_n(2, &mut out).expect("ok");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_error_rate_pct10_no_ops() {
        let m = ExofsMetrics::new_const();
        assert_eq!(m.error_rate_pct10(), 0);
    }
}
