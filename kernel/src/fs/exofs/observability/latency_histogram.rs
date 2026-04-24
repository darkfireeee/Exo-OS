//! latency_histogram.rs — Histogramme de latences ExoFS (no_std).
//!
//! Fournit :
//!  - `LatencyBucket`    : borne supérieure de bucket en µs.
//!  - `LatencyHistogram` : histogramme 16 buckets log2, atomique.
//!  - `LatencySummary`   : résumé (min, max, mean, p50, p90, p99).
//!  - `LatencyTracker`   : wrapper avec catégorie d'opération.
//!  - `LATENCY_HIST`     : singleton global.
//!
//! RECUR-01 : while uniquement.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Bornes des buckets ───────────────────────────────────────────────────────
//
// Buckets log2 en µs : [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024,
//                        4096, 16384, 65536, +inf]

pub const NUM_BUCKETS: usize = 16;

/// Bornes supérieures des buckets en µs (dernier = u64::MAX = overflow).
pub const BUCKET_UPPER_US: [u64; NUM_BUCKETS] = [
    1,
    2,
    4,
    8,
    16,
    32,
    64,
    128,
    256,
    512,
    1_024,
    4_096,
    16_384,
    65_536,
    262_144,
    u64::MAX,
];

/// Retourne l'index du bucket pour une latence en µs.
pub fn bucket_index(us: u64) -> usize {
    let mut i = 0usize;
    while i < NUM_BUCKETS.saturating_sub(1) {
        if us <= BUCKET_UPPER_US[i] {
            return i;
        }
        i = i.wrapping_add(1);
    }
    NUM_BUCKETS.saturating_sub(1)
}

// ─── LatencyHistogram ────────────────────────────────────────────────────────

/// Histogramme de latences atomique.
pub struct LatencyHistogram {
    buckets: [AtomicU64; NUM_BUCKETS],
    count: AtomicU64,
    sum_us: AtomicU64,
    min_us: AtomicU64,
    max_us: AtomicU64,
}

// SAFETY : AtomicU64 est Sync+Send.
unsafe impl Sync for LatencyHistogram {}
unsafe impl Send for LatencyHistogram {}

impl LatencyHistogram {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            buckets: [Z; NUM_BUCKETS],
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            min_us: AtomicU64::new(u64::MAX),
            max_us: AtomicU64::new(0),
        }
    }

    /// Enregistre une latence en µs.
    pub fn record(&self, us: u64) {
        let idx = bucket_index(us);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
        // min (CAS loop — RECUR-01 : while)
        let mut cur_min = self.min_us.load(Ordering::Relaxed);
        while us < cur_min {
            match self.min_us.compare_exchange_weak(
                cur_min,
                us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur_min = v,
            }
        }
        // max
        let mut cur_max = self.max_us.load(Ordering::Relaxed);
        while us > cur_max {
            match self.max_us.compare_exchange_weak(
                cur_max,
                us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur_max = v,
            }
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
    pub fn sum_us(&self) -> u64 {
        self.sum_us.load(Ordering::Relaxed)
    }
    pub fn min_us(&self) -> u64 {
        let v = self.min_us.load(Ordering::Relaxed);
        if v == u64::MAX {
            0
        } else {
            v
        }
    }
    pub fn max_us(&self) -> u64 {
        self.max_us.load(Ordering::Relaxed)
    }

    /// Moyenne en µs (ARITH-02).
    pub fn mean_us(&self) -> u64 {
        let c = self.count.load(Ordering::Relaxed);
        self.sum_us
            .load(Ordering::Relaxed)
            .checked_div(c)
            .unwrap_or(0)
    }

    /// Copie atomique des compteurs de buckets.
    pub fn bucket_snapshot(&self) -> [u64; NUM_BUCKETS] {
        let mut snap = [0u64; NUM_BUCKETS];
        let mut i = 0usize;
        while i < NUM_BUCKETS {
            snap[i] = self.buckets[i].load(Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
        snap
    }

    /// Estime un percentile (0–100) depuis les buckets (RECUR-01 : while).
    pub fn percentile_us(&self, pct: u8) -> u64 {
        let total = self.count();
        if total == 0 {
            return 0;
        }
        let target = total
            .saturating_mul(pct as u64)
            .checked_div(100)
            .unwrap_or(0);
        let snap = self.bucket_snapshot();
        let mut cumul = 0u64;
        let mut i = 0usize;
        while i < NUM_BUCKETS {
            cumul = cumul.saturating_add(snap[i]);
            if cumul >= target {
                return BUCKET_UPPER_US[i];
            }
            i = i.wrapping_add(1);
        }
        BUCKET_UPPER_US[NUM_BUCKETS.saturating_sub(1)]
    }

    /// Résumé complet.
    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            count: self.count(),
            sum_us: self.sum_us(),
            min_us: self.min_us(),
            max_us: self.max_us(),
            mean_us: self.mean_us(),
            p50_us: self.percentile_us(50),
            p90_us: self.percentile_us(90),
            p99_us: self.percentile_us(99),
            buckets: self.bucket_snapshot(),
        }
    }

    /// Remet le compteur à zéro.
    pub fn reset(&self) {
        let mut i = 0usize;
        while i < NUM_BUCKETS {
            self.buckets[i].store(0, Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
        self.count.store(0, Ordering::Relaxed);
        self.sum_us.store(0, Ordering::Relaxed);
        self.min_us.store(u64::MAX, Ordering::Relaxed);
        self.max_us.store(0, Ordering::Relaxed);
    }
}

pub static LATENCY_HIST: LatencyHistogram = LatencyHistogram::new_const();

// ─── LatencySummary ──────────────────────────────────────────────────────────

/// Résumé complet d'un histogramme.
#[derive(Clone, Copy, Debug, Default)]
pub struct LatencySummary {
    pub count: u64,
    pub sum_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p99_us: u64,
    pub buckets: [u64; NUM_BUCKETS],
}

impl LatencySummary {
    /// Retourne l'index du bucket le plus chargé (RECUR-01 : while).
    pub fn peak_bucket(&self) -> usize {
        let mut max_val = 0u64;
        let mut max_idx = 0usize;
        let mut i = 0usize;
        while i < NUM_BUCKETS {
            if self.buckets[i] > max_val {
                max_val = self.buckets[i];
                max_idx = i;
            }
            i = i.wrapping_add(1);
        }
        max_idx
    }

    /// True si la latence p99 est sous le seuil donné.
    pub fn p99_ok(&self, threshold_us: u64) -> bool {
        self.p99_us <= threshold_us
    }
}

// ─── LatencyTracker ──────────────────────────────────────────────────────────

/// Wrapper multi-catégorie (read/write/flush/other).
pub struct LatencyTracker {
    read: LatencyHistogram,
    write: LatencyHistogram,
    flush: LatencyHistogram,
    other: LatencyHistogram,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatencyCategory {
    Read,
    Write,
    Flush,
    Other,
}

impl LatencyTracker {
    pub const fn new_const() -> Self {
        Self {
            read: LatencyHistogram::new_const(),
            write: LatencyHistogram::new_const(),
            flush: LatencyHistogram::new_const(),
            other: LatencyHistogram::new_const(),
        }
    }

    pub fn record(&self, cat: LatencyCategory, us: u64) {
        match cat {
            LatencyCategory::Read => self.read.record(us),
            LatencyCategory::Write => self.write.record(us),
            LatencyCategory::Flush => self.flush.record(us),
            LatencyCategory::Other => self.other.record(us),
        }
    }

    pub fn summary(&self, cat: LatencyCategory) -> LatencySummary {
        match cat {
            LatencyCategory::Read => self.read.summary(),
            LatencyCategory::Write => self.write.summary(),
            LatencyCategory::Flush => self.flush.summary(),
            LatencyCategory::Other => self.other.summary(),
        }
    }

    pub fn reset_all(&self) {
        self.read.reset();
        self.write.reset();
        self.flush.reset();
        self.other.reset();
    }

    /// Retourne true si toutes les catégories ont p99 ≤ seuil.
    pub fn all_p99_ok(&self, threshold_us: u64) -> bool {
        self.read.percentile_us(99) <= threshold_us
            && self.write.percentile_us(99) <= threshold_us
            && self.flush.percentile_us(99) <= threshold_us
    }
}

pub static LATENCY_TRACKER: LatencyTracker = LatencyTracker::new_const();

// ─── LatencyWindow ────────────────────────────────────────────────────────────
// Ring de mesures récentes pour calcul de tendance.

pub const LATENCY_WINDOW_SIZE: usize = 32;

pub struct LatencyWindow {
    samples: [AtomicU64; LATENCY_WINDOW_SIZE],
    head: AtomicU64,
    count: AtomicU64,
}

unsafe impl Sync for LatencyWindow {}

impl LatencyWindow {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            samples: [Z; LATENCY_WINDOW_SIZE],
            head: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn push(&self, us: u64) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % LATENCY_WINDOW_SIZE;
        self.samples[idx].store(us, Ordering::Relaxed);
        let c = self.count.load(Ordering::Relaxed);
        if c < LATENCY_WINDOW_SIZE as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Moyenne glissante (ARITH-02 / RECUR-01).
    pub fn rolling_mean_us(&self) -> u64 {
        let n = self.count.load(Ordering::Relaxed);
        if n == 0 {
            return 0;
        }
        let mut sum = 0u64;
        let mut i = 0usize;
        while i < n as usize {
            sum = sum.saturating_add(self.samples[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        sum.checked_div(n).unwrap_or(0)
    }

    /// Copie les échantillons occupés dans un Vec (OOM-02).
    pub fn to_vec(&self) -> ExofsResult<Vec<u64>> {
        let n = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n {
            let idx = (head
                .wrapping_add(LATENCY_WINDOW_SIZE)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % LATENCY_WINDOW_SIZE;
            v.push(self.samples[idx].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bucket_index() {
        assert_eq!(bucket_index(0), 0);
        assert_eq!(bucket_index(1), 0);
        assert_eq!(bucket_index(2), 1);
        assert_eq!(bucket_index(3), 2);
        assert_eq!(bucket_index(1000), 10);
    }

    #[test]
    fn test_record_and_count() {
        let h = LatencyHistogram::new_const();
        h.record(10);
        h.record(200);
        assert_eq!(h.count(), 2);
    }

    #[test]
    fn test_min_max() {
        let h = LatencyHistogram::new_const();
        h.record(50);
        h.record(500);
        h.record(5);
        assert_eq!(h.min_us(), 5);
        assert_eq!(h.max_us(), 500);
    }

    #[test]
    fn test_mean() {
        let h = LatencyHistogram::new_const();
        h.record(100);
        h.record(300);
        assert_eq!(h.mean_us(), 200);
    }

    #[test]
    fn test_percentile_p50() {
        let h = LatencyHistogram::new_const();
        // 4 mesures dans le bucket 0 (<=1µs) et 4 dans bucket 4 (<=16µs)
        let mut i = 0usize;
        while i < 4 {
            h.record(1);
            i = i.wrapping_add(1);
        }
        let mut j = 0usize;
        while j < 4 {
            h.record(16);
            j = j.wrapping_add(1);
        }
        let p50 = h.percentile_us(50);
        assert!(p50 <= 16);
    }

    #[test]
    fn test_reset() {
        let h = LatencyHistogram::new_const();
        h.record(100);
        h.record(200);
        h.reset();
        assert_eq!(h.count(), 0);
        assert_eq!(h.min_us(), 0);
    }

    #[test]
    fn test_summary_peak_bucket() {
        let h = LatencyHistogram::new_const();
        let mut i = 0usize;
        while i < 5 {
            h.record(500);
            i = i.wrapping_add(1);
        }
        h.record(1);
        let s = h.summary();
        let peak = s.peak_bucket();
        assert_eq!(s.buckets[peak], 5);
    }

    #[test]
    fn test_tracker_categories() {
        let t = LatencyTracker::new_const();
        t.record(LatencyCategory::Read, 10);
        t.record(LatencyCategory::Write, 5000);
        assert_eq!(t.summary(LatencyCategory::Read).count, 1);
        assert_eq!(t.summary(LatencyCategory::Write).count, 1);
    }

    #[test]
    fn test_tracker_all_p99_ok() {
        let t = LatencyTracker::new_const();
        t.record(LatencyCategory::Read, 10);
        t.record(LatencyCategory::Write, 10);
        t.record(LatencyCategory::Flush, 10);
        assert!(t.all_p99_ok(100));
    }

    #[test]
    fn test_window_rolling_mean() {
        let w = LatencyWindow::new_const();
        w.push(100);
        w.push(200);
        w.push(300);
        assert_eq!(w.rolling_mean_us(), 200);
    }

    #[test]
    fn test_window_to_vec() {
        let w = LatencyWindow::new_const();
        w.push(10);
        w.push(20);
        let v = w.to_vec().expect("ok");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_p99_ok_fn() {
        let s = LatencySummary {
            p99_us: 50,
            ..Default::default()
        };
        assert!(s.p99_ok(100));
        assert!(!s.p99_ok(49));
    }
}
