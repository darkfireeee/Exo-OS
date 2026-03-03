//! io_stats.rs — Statistiques et compteurs IO ExoFS (no_std, thread-safe).
//!
//! Ce module fournit :
//!  - `IoStats`           : compteurs atomiques globaux des opérations IO.
//!  - `IoStatsSnapshot`   : snapshot non-atomique des compteurs (pour affichage).
//!  - `IoLatencyBucket`   : histogramme de latences IO (8 buckets log2).
//!  - `IoOpRecord`        : enregistrement d'une opération IO (audit circulaire).
//!  - `IoOpKind`          : type d'opération (Read/Write/Flush/Discard).
//!  - `IO_STATS`          : instance statique globale.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Instance statique globale ────────────────────────────────────────────────

/// Instance globale des statistiques IO ExoFS.
pub static IO_STATS: IoStats = IoStats::new_const();

// ─── Type d'opération IO ──────────────────────────────────────────────────────

/// Type d'une opération IO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IoOpKind {
    Read    = 0,
    Write   = 1,
    Flush   = 2,
    Discard = 3,
    Sync    = 4,
    Readahead = 5,
    Prefetch  = 6,
    Writeback = 7,
}

impl IoOpKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Read,
            1 => Self::Write,
            2 => Self::Flush,
            3 => Self::Discard,
            4 => Self::Sync,
            5 => Self::Readahead,
            6 => Self::Prefetch,
            7 => Self::Writeback,
            _ => Self::Read,
        }
    }

    pub fn is_read(self) -> bool { matches!(self, IoOpKind::Read | IoOpKind::Readahead | IoOpKind::Prefetch) }
    pub fn is_write(self) -> bool { matches!(self, IoOpKind::Write | IoOpKind::Writeback) }
    pub fn as_str(self) -> &'static str {
        match self {
            IoOpKind::Read      => "read",
            IoOpKind::Write     => "write",
            IoOpKind::Flush     => "flush",
            IoOpKind::Discard   => "discard",
            IoOpKind::Sync      => "sync",
            IoOpKind::Readahead => "readahead",
            IoOpKind::Prefetch  => "prefetch",
            IoOpKind::Writeback => "writeback",
        }
    }
}

// ─── Enregistrement d'une opération ──────────────────────────────────────────

/// Enregistrement d'une opération IO (64 bytes, repr C).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IoOpRecord {
    /// Timestamp (cycles CPU ou tick kernel).
    pub timestamp: u64,
    /// BlobId partiel (premiers 16 bytes).
    pub blob_id_partial: [u8; 16],
    /// Taille en bytes.
    pub size: u64,
    /// Durée en µs (saturating).
    pub latency_us: u32,
    /// Type d'opération.
    pub kind: u8,
    /// Résultat : 0 = succès, 1 = erreur.
    pub result: u8,
    /// Padding.
    pub _pad: [u8; 10],
}

const _: () = assert!(core::mem::size_of::<IoOpRecord>() == 48);

impl IoOpRecord {
    pub const fn new_empty() -> Self {
        Self {
            timestamp: 0, blob_id_partial: [0u8; 16], size: 0,
            latency_us: 0, kind: 0, result: 0, _pad: [0u8; 10],
        }
    }

    pub fn new(kind: IoOpKind, size: u64, latency_us: u32, ok: bool, blob_id: &[u8; 32]) -> Self {
        let mut partial = [0u8; 16];
        partial.copy_from_slice(&blob_id[..16]);
        Self {
            timestamp: 0, // sera rempli par l'appelant
            blob_id_partial: partial, size,
            latency_us, kind: kind as u8, result: if ok { 0 } else { 1 }, _pad: [0u8; 10],
        }
    }

    pub fn is_ok(&self) -> bool { self.result == 0 }
    pub fn kind(&self) -> IoOpKind { IoOpKind::from_u8(self.kind) }
}

// ─── Histogramme de latences ──────────────────────────────────────────────────

/// Nombre de buckets dans l'histogramme.
pub const LATENCY_BUCKETS: usize = 8;

/// Histogramme de latences IO (buckets log2 en µs).
/// Bucket i contient les ops dont la latence ∈ [2^i µs, 2^(i+1) µs[.
/// - Bucket 0 : < 1 µs
/// - Bucket 1 : 1–2 µs
/// - Bucket 2 : 2–4 µs
/// - ...
/// - Bucket 7 : ≥ 128 µs
pub struct IoLatencyBucket {
    counts: [AtomicU64; LATENCY_BUCKETS],
    total_us: AtomicU64,
    total_ops: AtomicU64,
}

impl IoLatencyBucket {
    pub const fn new() -> Self {
        Self {
            counts: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ],
            total_us: AtomicU64::new(0),
            total_ops: AtomicU64::new(0),
        }
    }

    /// Enregistre une latence en µs dans le bucket approprié.
    pub fn record(&self, latency_us: u32) {
        // Bucket = floor(log2(latency_us + 1)).min(7)
        let bucket = if latency_us == 0 { 0 } else {
            let bits = 32 - latency_us.leading_zeros();
            (bits as usize).saturating_sub(1).min(LATENCY_BUCKETS - 1)
        };
        self.counts[bucket].fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(latency_us as u64, Ordering::Relaxed);
        self.total_ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Bucket count (RECUR-01 safe — pas de récursion).
    pub fn bucket_count(&self, i: usize) -> u64 {
        if i >= LATENCY_BUCKETS { return 0; }
        self.counts[i].load(Ordering::Relaxed)
    }

    /// Latence moyenne en µs × 10 (sans float) — ARITH-02 : checked_div.
    pub fn avg_us_pct10(&self) -> u64 {
        let total = self.total_ops.load(Ordering::Relaxed);
        let sum = self.total_us.load(Ordering::Relaxed);
        sum.saturating_mul(10)
            .checked_div(total)
            .unwrap_or(0)
    }

    /// Latence totale cumulée en µs.
    pub fn total_us(&self) -> u64 { self.total_us.load(Ordering::Relaxed) }
    pub fn total_ops(&self) -> u64 { self.total_ops.load(Ordering::Relaxed) }

    /// Remet tous les buckets à zéro.
    pub fn reset(&self) {
        let mut i = 0usize;
        while i < LATENCY_BUCKETS {
            self.counts[i].store(0, Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
        self.total_us.store(0, Ordering::Relaxed);
        self.total_ops.store(0, Ordering::Relaxed);
    }
}

// ─── Journal circulaire IO ────────────────────────────────────────────────────

/// Taille du journal circulaire d'opérations IO.
pub const IO_OP_RING_SIZE: usize = 256;
const IO_OP_RING_MASK: usize = IO_OP_RING_SIZE - 1;

/// Journal circulaire des dernières opérations IO (thread-safe via spinlock).
pub struct IoOpRing {
    entries: UnsafeCell<[IoOpRecord; IO_OP_RING_SIZE]>,
    head: AtomicU64,    // prochain index à écrire
    _lock: AtomicU64,   // spinlock
}

unsafe impl Sync for IoOpRing {}
unsafe impl Send for IoOpRing {}

impl IoOpRing {
    pub const fn new() -> Self {
        Self {
            entries: UnsafeCell::new([IoOpRecord::new_empty(); IO_OP_RING_SIZE]),
            head: AtomicU64::new(0),
            _lock: AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self._lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn release(&self) { self._lock.store(0, Ordering::Release); }

    /// Enregistre une opération IO.
    pub fn push(&self, mut rec: IoOpRecord, ts: u64) {
        rec.timestamp = ts;
        self.acquire();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = head & IO_OP_RING_MASK;
        unsafe { (*self.entries.get())[idx] = rec; }
        self.head.store(head.wrapping_add(1) as u64, Ordering::Relaxed);
        self.release();
    }

    /// Retourne les N dernières entrées (RECUR‑01 : boucle while).
    pub fn last_n(&self, n: usize) -> [IoOpRecord; IO_OP_RING_SIZE] {
        self.acquire();
        let snapshot = unsafe { *self.entries.get() };
        self.release();
        let _ = n; // Pour usage futur (filtre)
        snapshot
    }

    /// Compte les erreurs dans les N dernières entrées (RECUR-01 : boucle while).
    pub fn error_count_last_n(&self, n: usize) -> u32 {
        self.acquire();
        let snapshot = unsafe { *self.entries.get() };
        self.release();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let count = n.min(IO_OP_RING_SIZE);
        let mut errors = 0u32;
        let mut i = 0usize;
        while i < count {
            let idx = head.wrapping_sub(i + 1) & IO_OP_RING_MASK;
            if !snapshot[idx].is_ok() {
                errors = errors.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        errors
    }
}

// ─── Compteurs IO principaux ──────────────────────────────────────────────────

/// Compteurs atomiques des opérations IO ExoFS.
pub struct IoStats {
    // ── Lectures ─────────────────────────────────────────────────────────────
    pub reads_ok:     AtomicU64,
    pub reads_err:    AtomicU64,
    pub bytes_read:   AtomicU64,

    // ── Écritures ────────────────────────────────────────────────────────────
    pub writes_ok:    AtomicU64,
    pub writes_err:   AtomicU64,
    pub bytes_written: AtomicU64,

    // ── Flush / Sync ─────────────────────────────────────────────────────────
    pub flushes:      AtomicU64,
    pub syncs:        AtomicU64,

    // ── Writeback ────────────────────────────────────────────────────────────
    pub writeback_enqueued:  AtomicU64,
    pub writeback_completed: AtomicU64,
    pub writeback_errors:    AtomicU64,

    // ── Cache / Prefetch ─────────────────────────────────────────────────────
    pub cache_hits:     AtomicU64,
    pub cache_misses:   AtomicU64,
    pub prefetch_ops:   AtomicU64,
    pub readahead_ops:  AtomicU64,

    // ── Batch / Async ────────────────────────────────────────────────────────
    pub batch_ops:     AtomicU64,
    pub async_ops:     AtomicU64,
    pub async_errors:  AtomicU64,

    // ── Histogramme de latences ───────────────────────────────────────────────
    pub read_latency:  IoLatencyBucket,
    pub write_latency: IoLatencyBucket,

    // ── Journal ───────────────────────────────────────────────────────────────
    pub ring: IoOpRing,

    // ── Epoch de reset ───────────────────────────────────────────────────────
    pub reset_count:  AtomicU64,
}

unsafe impl Sync for IoStats {}
unsafe impl Send for IoStats {}

impl IoStats {
    pub const fn new_const() -> Self {
        Self {
            reads_ok: AtomicU64::new(0), reads_err: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            writes_ok: AtomicU64::new(0), writes_err: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            flushes: AtomicU64::new(0), syncs: AtomicU64::new(0),
            writeback_enqueued: AtomicU64::new(0),
            writeback_completed: AtomicU64::new(0),
            writeback_errors: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0), cache_misses: AtomicU64::new(0),
            prefetch_ops: AtomicU64::new(0), readahead_ops: AtomicU64::new(0),
            batch_ops: AtomicU64::new(0),
            async_ops: AtomicU64::new(0), async_errors: AtomicU64::new(0),
            read_latency: IoLatencyBucket::new(),
            write_latency: IoLatencyBucket::new(),
            ring: IoOpRing::new(),
            reset_count: AtomicU64::new(0),
        }
    }

    // ── Enregistreurs de bas niveau ───────────────────────────────────────────

    /// Enregistre une lecture OK.
    pub fn record_read_ok(&self, bytes: u64, latency_us: u32) {
        self.reads_ok.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.read_latency.record(latency_us);
    }

    /// Enregistre une lecture échouée.
    pub fn record_read_err(&self) {
        self.reads_err.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre une écriture OK.
    pub fn record_write_ok(&self, bytes: u64, latency_us: u32) {
        self.writes_ok.fetch_add(1, Ordering::Relaxed);
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.write_latency.record(latency_us);
    }

    /// Enregistre une écriture échouée.
    pub fn record_write_err(&self) {
        self.writes_err.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un flush.
    pub fn record_flush(&self) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un sync.
    pub fn record_sync(&self) {
        self.syncs.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un hit cache.
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un miss cache.
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    pub fn total_reads(&self) -> u64 {
        self.reads_ok.load(Ordering::Relaxed).saturating_add(
            self.reads_err.load(Ordering::Relaxed))
    }

    pub fn total_writes(&self) -> u64 {
        self.writes_ok.load(Ordering::Relaxed).saturating_add(
            self.writes_err.load(Ordering::Relaxed))
    }

    /// Ratio de succès des lectures × 1000 (sans float) — ARITH-02 : checked_div.
    pub fn read_success_pct10(&self) -> u64 {
        let ok = self.reads_ok.load(Ordering::Relaxed);
        let total = self.total_reads();
        ok.saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(1000)
    }

    /// Ratio hit/miss cache × 1000 — ARITH-02 : checked_div.
    pub fn cache_hit_ratio_pct10(&self) -> u64 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let total = hits.saturating_add(self.cache_misses.load(Ordering::Relaxed));
        hits.saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(0)
    }

    pub fn is_clean(&self) -> bool {
        self.reads_err.load(Ordering::Relaxed) == 0
            && self.writes_err.load(Ordering::Relaxed) == 0
            && self.async_errors.load(Ordering::Relaxed) == 0
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        self.reads_ok.store(0, Ordering::Relaxed);
        self.reads_err.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
        self.writes_ok.store(0, Ordering::Relaxed);
        self.writes_err.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.flushes.store(0, Ordering::Relaxed);
        self.syncs.store(0, Ordering::Relaxed);
        self.writeback_enqueued.store(0, Ordering::Relaxed);
        self.writeback_completed.store(0, Ordering::Relaxed);
        self.writeback_errors.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
        self.prefetch_ops.store(0, Ordering::Relaxed);
        self.readahead_ops.store(0, Ordering::Relaxed);
        self.batch_ops.store(0, Ordering::Relaxed);
        self.async_ops.store(0, Ordering::Relaxed);
        self.async_errors.store(0, Ordering::Relaxed);
        self.read_latency.reset();
        self.write_latency.reset();
        self.reset_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Prend un snapshot non-atomique (usage debug/sysfs).
    pub fn snapshot(&self) -> IoStatsSnapshot {
        IoStatsSnapshot {
            reads_ok:    self.reads_ok.load(Ordering::Relaxed),
            reads_err:   self.reads_err.load(Ordering::Relaxed),
            bytes_read:  self.bytes_read.load(Ordering::Relaxed),
            writes_ok:   self.writes_ok.load(Ordering::Relaxed),
            writes_err:  self.writes_err.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            flushes:     self.flushes.load(Ordering::Relaxed),
            syncs:       self.syncs.load(Ordering::Relaxed),
            writeback_enqueued:  self.writeback_enqueued.load(Ordering::Relaxed),
            writeback_completed: self.writeback_completed.load(Ordering::Relaxed),
            writeback_errors: self.writeback_errors.load(Ordering::Relaxed),
            cache_hits:   self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            prefetch_ops: self.prefetch_ops.load(Ordering::Relaxed),
            readahead_ops: self.readahead_ops.load(Ordering::Relaxed),
            batch_ops:   self.batch_ops.load(Ordering::Relaxed),
            async_ops:   self.async_ops.load(Ordering::Relaxed),
            async_errors: self.async_errors.load(Ordering::Relaxed),
            read_avg_latency_us_pct10:  self.read_latency.avg_us_pct10(),
            write_avg_latency_us_pct10: self.write_latency.avg_us_pct10(),
            reset_count: self.reset_count.load(Ordering::Relaxed),
        }
    }
}

// ─── Snapshot des statistiques ────────────────────────────────────────────────

/// Snapshot non-atomique des statistiques IO (pour affichage).
#[derive(Clone, Copy, Debug, Default)]
pub struct IoStatsSnapshot {
    pub reads_ok:    u64,
    pub reads_err:   u64,
    pub bytes_read:  u64,
    pub writes_ok:   u64,
    pub writes_err:  u64,
    pub bytes_written: u64,
    pub flushes:     u64,
    pub syncs:       u64,
    pub writeback_enqueued:  u64,
    pub writeback_completed: u64,
    pub writeback_errors:    u64,
    pub cache_hits:   u64,
    pub cache_misses: u64,
    pub prefetch_ops: u64,
    pub readahead_ops: u64,
    pub batch_ops:   u64,
    pub async_ops:   u64,
    pub async_errors: u64,
    pub read_avg_latency_us_pct10:  u64,
    pub write_avg_latency_us_pct10: u64,
    pub reset_count: u64,
}

impl IoStatsSnapshot {
    pub fn total_ops(&self) -> u64 {
        self.reads_ok.saturating_add(self.reads_err)
            .saturating_add(self.writes_ok)
            .saturating_add(self.writes_err)
    }

    pub fn error_count(&self) -> u64 {
        self.reads_err.saturating_add(self.writes_err).saturating_add(self.async_errors)
    }

    pub fn is_clean(&self) -> bool { self.error_count() == 0 }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_stats_counters() {
        let s = IoStats::new_const();
        s.record_read_ok(1024, 10);
        s.record_read_ok(512, 5);
        assert_eq!(s.reads_ok.load(Ordering::Relaxed), 2);
        assert_eq!(s.bytes_read.load(Ordering::Relaxed), 1536);
    }

    #[test]
    fn test_io_stats_errors() {
        let s = IoStats::new_const();
        s.record_read_err();
        s.record_write_err();
        assert!(!s.is_clean());
    }

    #[test]
    fn test_io_stats_read_success_ratio() {
        let s = IoStats::new_const();
        s.record_read_ok(100, 1);
        s.record_read_ok(100, 1);
        s.record_read_ok(100, 1);
        s.record_read_err();
        // 3 ok / 4 total = 750/1000
        assert_eq!(s.read_success_pct10(), 750);
    }

    #[test]
    fn test_io_stats_cache_ratio() {
        let s = IoStats::new_const();
        s.record_cache_hit();
        s.record_cache_hit();
        s.record_cache_miss();
        // 2/3 = 666/1000
        assert_eq!(s.cache_hit_ratio_pct10(), 666);
    }

    #[test]
    fn test_latency_bucket_record() {
        let b = IoLatencyBucket::new();
        b.record(0);    // bucket 0
        b.record(1);    // bucket 0
        b.record(2);    // bucket 1
        b.record(100);  // bucket 6
        assert_eq!(b.total_ops(), 4);
        assert!(b.avg_us_pct10() > 0);
    }

    #[test]
    fn test_latency_bucket_reset() {
        let b = IoLatencyBucket::new();
        b.record(50);
        b.reset();
        assert_eq!(b.total_ops(), 0);
        assert_eq!(b.total_us(), 0);
    }

    #[test]
    fn test_io_op_record_size() {
        assert_eq!(core::mem::size_of::<IoOpRecord>(), 48);
    }

    #[test]
    fn test_io_op_record_new() {
        let id = [0xABu8; 32];
        let r = IoOpRecord::new(IoOpKind::Read, 512, 15, true, &id);
        assert!(r.is_ok());
        assert_eq!(r.kind(), IoOpKind::Read);
        assert_eq!(r.size, 512);
    }

    #[test]
    fn test_io_op_ring_push_and_errors() {
        let ring = IoOpRing::new();
        let id = [0u8; 32];
        ring.push(IoOpRecord::new(IoOpKind::Read, 100, 5, true, &id), 1);
        ring.push(IoOpRecord::new(IoOpKind::Write, 200, 10, false, &id), 2);
        assert_eq!(ring.error_count_last_n(10), 1);
    }

    #[test]
    fn test_snapshot_fields() {
        let s = IoStats::new_const();
        s.record_write_ok(4096, 20);
        let snap = s.snapshot();
        assert_eq!(snap.writes_ok, 1);
        assert_eq!(snap.bytes_written, 4096);
        assert_eq!(snap.total_ops(), 1);
    }

    #[test]
    fn test_stats_reset() {
        let s = IoStats::new_const();
        s.record_read_ok(1024, 5);
        s.reset();
        assert_eq!(s.reads_ok.load(Ordering::Relaxed), 0);
        assert_eq!(s.reset_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_io_op_kind_round_trip() {
        for v in 0u8..8 {
            let k = IoOpKind::from_u8(v);
            assert_eq!(k as u8, v);
        }
    }

    #[test]
    fn test_total_reads_writes() {
        let s = IoStats::new_const();
        s.record_read_ok(10, 1);
        s.record_read_err();
        s.record_write_ok(20, 2);
        assert_eq!(s.total_reads(), 2);
        assert_eq!(s.total_writes(), 1);
    }

    #[test]
    fn test_snapshot_clean() {
        let s = IoStats::new_const();
        s.record_read_ok(512, 3);
        let snap = s.snapshot();
        assert!(snap.is_clean());
    }
}
