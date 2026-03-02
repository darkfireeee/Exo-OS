// kernel/src/scheduler/stats/latency.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Histogramme de latence — mesures p50/p99/p999 via buckets AtomicU64
// ═══════════════════════════════════════════════════════════════════════════════
//
// Buckets log2 : chaque bucket k couvre [2^k ns, 2^(k+1) ns).
//   Bucket 0 : [0, 1ns)
//   Bucket 10 : [1µs, 2µs)
//   Bucket 20 : [1ms, 2ms)
//   Bucket 30 : [1s, 2s)
//
// BUCKETS = 40 (couvre 0–1100s, plus que suffisant pour la latence scheduler).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

const BUCKETS: usize = 40;

/// Histogramme de latence global (un seul pour tout l'OS).
pub struct LatencyHist {
    buckets: [AtomicU64; BUCKETS],
    total:   AtomicU64,
    sum_ns:  AtomicU64,
    max_ns:  AtomicU64,
}

impl LatencyHist {
    const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            buckets: [ZERO; BUCKETS],
            total:   AtomicU64::new(0),
            sum_ns:  AtomicU64::new(0),
            max_ns:  AtomicU64::new(0),
        }
    }

    /// Enregistre une mesure de latence (en nanosecondes).
    pub fn record(&self, ns: u64) {
        let bucket = bucket_for(ns).min(BUCKETS - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        // Mise à jour du max.
        let mut cur_max = self.max_ns.load(Ordering::Relaxed);
        while ns > cur_max {
            match self.max_ns.compare_exchange_weak(
                cur_max, ns, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(m) => cur_max = m,
            }
        }
    }

    /// Percentile P (0–100). Retourne la borne inférieure du bucket correspondant.
    pub fn percentile_pct(&self, p: u64) -> u64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        let threshold = (total * p + 99) / 100;
        let mut cumul = 0u64;
        for i in 0..BUCKETS {
            cumul += self.buckets[i].load(Ordering::Relaxed);
            if cumul >= threshold {
                return bucket_floor(i);
            }
        }
        bucket_floor(BUCKETS - 1)
    }

    pub fn p50(&self)  -> u64 { self.percentile_pct(50) }
    pub fn p99(&self)  -> u64 { self.percentile_pct(99) }
    /// 99.9th percentile (uses millipercent scale 0–1000).
    pub fn p999(&self) -> u64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        let threshold = (total * 999 + 999) / 1000;
        let mut cumul = 0u64;
        for i in 0..BUCKETS {
            cumul += self.buckets[i].load(Ordering::Relaxed);
            if cumul >= threshold { return bucket_floor(i); }
        }
        bucket_floor(BUCKETS - 1)
    }

    pub fn total(&self)  -> u64 { self.total.load(Ordering::Relaxed) }
    pub fn max_ns(&self) -> u64 { self.max_ns.load(Ordering::Relaxed) }
    pub fn avg_ns(&self) -> u64 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 { 0 } else { self.sum_ns.load(Ordering::Relaxed) / t }
    }

    pub fn reset(&self) {
        for b in &self.buckets { b.store(0, Ordering::Relaxed); }
        self.total.store(0, Ordering::Relaxed);
        self.sum_ns.store(0, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
    }
}

/// Retourne le numéro de bucket pour une latence `ns`.
/// Bucket k couvre [bucket_floor(k), bucket_floor(k+1)) où bucket_floor(k) = 2^(k-1) pour k>=1.
/// Formule : k = nombre de bits de `ns` = `u64::BITS - ns.leading_zeros()`.
/// Exemples : ns=0→0, ns=1→1, ns=2..3→2, ns=4..7→3, ns=8..15→4.
#[inline]
fn bucket_for(ns: u64) -> usize {
    if ns == 0 { 0 } else { (u64::BITS - ns.leading_zeros()) as usize }
}

/// Retourne la borne inférieure du bucket `k` (en ns).
#[inline]
fn bucket_floor(k: usize) -> u64 {
    if k == 0 { 0 } else { 1u64 << (k - 1) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Histogrammes globaux nommés
// ─────────────────────────────────────────────────────────────────────────────

/// Latence de context_switch (ns).
pub static SWITCH_LATENCY:  LatencyHist = LatencyHist::new();
/// Latence de wakeup (ns).
pub static WAKEUP_LATENCY:  LatencyHist = LatencyHist::new();
/// Latence pick_next_task (ns).
pub static PICKNEXT_LATENCY: LatencyHist = LatencyHist::new();
/// Latence IPI (ns).
pub static IPI_LATENCY:     LatencyHist = LatencyHist::new();

pub unsafe fn init() {
    // Histogrammes initialisés à la compilation — rien à faire.
}
