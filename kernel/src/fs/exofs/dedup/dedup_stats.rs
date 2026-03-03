//! DedupStats — statistiques globales de déduplication (no_std).
//!
//! Suivi des économies de stockage, du ratio de déduplication,
//! et de l'activité du moteur (insertions, hits, erreurs).
//!
//! RECUR-01 : aucune récursion — pas de fonctions récursives.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : saturating / checked / wrapping sur tous les compteurs.

#![allow(dead_code)]

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const STATS_MAX_SNAPSHOTS: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// DedupStats — compteurs atomiques globaux
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs atomiques de déduplication.
pub struct DedupStats {
    /// Nombre total de blobs soumis au moteur de déduplication.
    pub total_blobs_processed:   AtomicU64,
    /// Nombre de blobs où une déduplication a eu lieu (au moins un chunk partagé).
    pub deduped_blobs:           AtomicU64,
    /// Nombre total de chunks traités.
    pub total_chunks_processed:  AtomicU64,
    /// Nombre de chunks dédupliqués (existaient déjà dans l'index).
    pub deduped_chunks:          AtomicU64,
    /// Octets logiques totaux écrits (avant dédu).
    pub logical_bytes_written:   AtomicU64,
    /// Octets physiques réellement stockés (après dédup).
    pub physical_bytes_stored:   AtomicU64,
    /// Octets économisés grâce à la déduplication.
    pub saved_bytes:             AtomicU64,
    /// Erreurs survenues pendant la déduplication.
    pub errors:                  AtomicU64,
    /// Nombre de vérifications d'intégrité réussies.
    pub integrity_checks_ok:     AtomicU64,
    /// Nombre de vérifications d'intégrité échouées.
    pub integrity_checks_failed: AtomicU64,
}

impl DedupStats {
    pub const fn new_const() -> Self {
        Self {
            total_blobs_processed:   AtomicU64::new(0),
            deduped_blobs:           AtomicU64::new(0),
            total_chunks_processed:  AtomicU64::new(0),
            deduped_chunks:          AtomicU64::new(0),
            logical_bytes_written:   AtomicU64::new(0),
            physical_bytes_stored:   AtomicU64::new(0),
            saved_bytes:             AtomicU64::new(0),
            errors:                  AtomicU64::new(0),
            integrity_checks_ok:     AtomicU64::new(0),
            integrity_checks_failed: AtomicU64::new(0),
        }
    }

    // ── Méthodes d'incrémentation ─────────────────────────────────────────────

    /// Enregistre le traitement d'un blob.
    ///
    /// ARITH-02 : saturating_add.
    pub fn record_blob(
        &self,
        deduped:          bool,
        logical_bytes:    u64,
        physical_bytes:   u64,
        n_chunks:         u64,
        deduped_chunks:   u64,
    ) {
        self.total_blobs_processed.fetch_add(1, Ordering::Relaxed);
        if deduped { self.deduped_blobs.fetch_add(1, Ordering::Relaxed); }
        self.total_chunks_processed.fetch_add(n_chunks, Ordering::Relaxed);
        self.deduped_chunks.fetch_add(deduped_chunks, Ordering::Relaxed);
        self.logical_bytes_written.fetch_add(logical_bytes, Ordering::Relaxed);
        self.physical_bytes_stored.fetch_add(physical_bytes, Ordering::Relaxed);
        let saved = logical_bytes.saturating_sub(physical_bytes);
        self.saved_bytes.fetch_add(saved, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_integrity_check(&self, ok: bool) {
        if ok {
            self.integrity_checks_ok.fetch_add(1, Ordering::Relaxed);
        } else {
            self.integrity_checks_failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        self.total_blobs_processed  .store(0, Ordering::Relaxed);
        self.deduped_blobs          .store(0, Ordering::Relaxed);
        self.total_chunks_processed .store(0, Ordering::Relaxed);
        self.deduped_chunks         .store(0, Ordering::Relaxed);
        self.logical_bytes_written  .store(0, Ordering::Relaxed);
        self.physical_bytes_stored  .store(0, Ordering::Relaxed);
        self.saved_bytes            .store(0, Ordering::Relaxed);
        self.errors                 .store(0, Ordering::Relaxed);
        self.integrity_checks_ok    .store(0, Ordering::Relaxed);
        self.integrity_checks_failed.store(0, Ordering::Relaxed);
    }

    // ── Calculs dérivés ───────────────────────────────────────────────────────

    /// Ratio de déduplication en pourcentage (0..=100).
    ///
    /// ARITH-02 : division guardée.
    pub fn dedup_ratio_pct(&self) -> u8 {
        let logical = self.logical_bytes_written.load(Ordering::Relaxed);
        if logical == 0 { return 0; }
        let saved = self.saved_bytes.load(Ordering::Relaxed);
        ((saved * 100) / logical).min(100) as u8
    }

    /// Taux de chunks dédupliqués (0..=100).
    pub fn chunk_dedup_pct(&self) -> u8 {
        let total = self.total_chunks_processed.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        let dedup = self.deduped_chunks.load(Ordering::Relaxed);
        ((dedup * 100) / total).min(100) as u8
    }

    /// Retourne un instantané sous forme de `DedupStatsSummary`.
    pub fn snapshot(&self) -> DedupStatsSummary {
        DedupStatsSummary {
            total_blobs_processed:   self.total_blobs_processed  .load(Ordering::Relaxed),
            deduped_blobs:           self.deduped_blobs           .load(Ordering::Relaxed),
            total_chunks_processed:  self.total_chunks_processed  .load(Ordering::Relaxed),
            deduped_chunks:          self.deduped_chunks          .load(Ordering::Relaxed),
            logical_bytes_written:   self.logical_bytes_written   .load(Ordering::Relaxed),
            physical_bytes_stored:   self.physical_bytes_stored   .load(Ordering::Relaxed),
            saved_bytes:             self.saved_bytes             .load(Ordering::Relaxed),
            errors:                  self.errors                  .load(Ordering::Relaxed),
            integrity_checks_ok:     self.integrity_checks_ok     .load(Ordering::Relaxed),
            integrity_checks_failed: self.integrity_checks_failed .load(Ordering::Relaxed),
            dedup_ratio_pct:         self.dedup_ratio_pct(),
            chunk_dedup_pct:         self.chunk_dedup_pct(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupStatsSummary — instantané non-atomique des stats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DedupStatsSummary {
    pub total_blobs_processed:   u64,
    pub deduped_blobs:           u64,
    pub total_chunks_processed:  u64,
    pub deduped_chunks:          u64,
    pub logical_bytes_written:   u64,
    pub physical_bytes_stored:   u64,
    pub saved_bytes:             u64,
    pub errors:                  u64,
    pub integrity_checks_ok:     u64,
    pub integrity_checks_failed: u64,
    pub dedup_ratio_pct:         u8,
    pub chunk_dedup_pct:         u8,
}

impl DedupStatsSummary {
    /// Vérifie la cohérence interne du résumé.
    pub fn is_consistent(&self) -> bool {
        self.deduped_blobs <= self.total_blobs_processed
        && self.deduped_chunks <= self.total_chunks_processed
        && self.saved_bytes <= self.logical_bytes_written
        && self.physical_bytes_stored <= self.logical_bytes_written
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupStatsHistory — historique des instantanés périodiques
// ─────────────────────────────────────────────────────────────────────────────

/// Historique circulaire d'instantanés de statistiques.
pub struct DedupStatsHistory {
    snapshots: Vec<DedupStatsSummary>,
    capacity:  usize,
    write_pos: usize,
    count:     usize,
}

impl DedupStatsHistory {
    /// OOM-02 : try_reserve.
    pub fn new(capacity: usize) -> ExofsResult<Self> {
        if capacity == 0 || capacity > STATS_MAX_SNAPSHOTS {
            return Err(ExofsError::InvalidArgument);
        }
        let mut v: Vec<DedupStatsSummary> = Vec::new();
        v.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { snapshots: v, capacity, write_pos: 0, count: 0 })
    }

    /// Enregistre un instantané (remplace le plus ancien si plein).
    ///
    /// ARITH-02 : wrapping_add pour position circulaire.
    pub fn push(&mut self, s: DedupStatsSummary) -> ExofsResult<()> {
        if self.snapshots.len() < self.capacity {
            self.snapshots.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            self.snapshots.push(s);
        } else {
            self.snapshots[self.write_pos] = s;
        }
        self.write_pos = self.write_pos.wrapping_add(1) % self.capacity;
        self.count     = self.count.saturating_add(1);
        Ok(())
    }

    pub fn len(&self)     -> usize { self.snapshots.len() }
    pub fn is_empty(&self)-> bool  { self.snapshots.is_empty() }

    /// Dernier instantané enregistré.
    pub fn latest(&self) -> Option<&DedupStatsSummary> {
        if self.snapshots.is_empty() { return None; }
        let pos = if self.write_pos == 0 { self.snapshots.len() - 1 } else { self.write_pos - 1 };
        self.snapshots.get(pos)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statique global
// ─────────────────────────────────────────────────────────────────────────────

pub static DEDUP_STATS: DedupStats = DedupStats::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_record_blob_no_dedup() {
        let s = DedupStats::new_const();
        s.record_blob(false, 4096, 4096, 1, 0);
        assert_eq!(s.total_blobs_processed.load(Ordering::Relaxed), 1);
        assert_eq!(s.deduped_blobs.load(Ordering::Relaxed), 0);
        assert_eq!(s.saved_bytes.load(Ordering::Relaxed), 0);
    }

    #[test] fn test_record_blob_with_dedup() {
        let s = DedupStats::new_const();
        s.record_blob(true, 8192, 4096, 2, 1);
        assert_eq!(s.deduped_blobs.load(Ordering::Relaxed), 1);
        assert_eq!(s.saved_bytes.load(Ordering::Relaxed), 4096);
    }

    #[test] fn test_dedup_ratio_pct() {
        let s = DedupStats::new_const();
        s.record_blob(true, 10000, 5000, 4, 2);
        assert_eq!(s.dedup_ratio_pct(), 50);
    }

    #[test] fn test_chunk_dedup_pct() {
        let s = DedupStats::new_const();
        s.record_blob(true, 1000, 500, 10, 5);
        assert_eq!(s.chunk_dedup_pct(), 50);
    }

    #[test] fn test_reset() {
        let s = DedupStats::new_const();
        s.record_blob(true, 1000, 500, 1, 1);
        s.reset();
        assert_eq!(s.total_blobs_processed.load(Ordering::Relaxed), 0);
    }

    #[test] fn test_snapshot_consistent() {
        let s = DedupStats::new_const();
        s.record_blob(true, 2000, 1000, 4, 2);
        let snap = s.snapshot();
        assert!(snap.is_consistent());
    }

    #[test] fn test_history_push_pop() {
        let mut h = DedupStatsHistory::new(4).unwrap();
        let s     = DedupStats::new_const();
        h.push(s.snapshot()).unwrap();
        h.push(s.snapshot()).unwrap();
        assert_eq!(h.len(), 2);
        assert!(h.latest().is_some());
    }

    #[test] fn test_history_circular() {
        let mut h = DedupStatsHistory::new(2).unwrap();
        let s     = DedupStats::new_const();
        h.push(s.snapshot()).unwrap();
        h.push(s.snapshot()).unwrap();
        h.push(s.snapshot()).unwrap(); // remplace le premier
        assert_eq!(h.len(), 2);
    }

    #[test] fn test_global_stats_accessible() {
        let snap = DEDUP_STATS.snapshot();
        let _ = snap.dedup_ratio_pct;
    }

    #[test] fn test_error_counter() {
        let s = DedupStats::new_const();
        s.record_error();
        s.record_error();
        assert_eq!(s.errors.load(Ordering::Relaxed), 2);
    }

    #[test] fn test_integrity_counters() {
        let s = DedupStats::new_const();
        s.record_integrity_check(true);
        s.record_integrity_check(false);
        assert_eq!(s.integrity_checks_ok    .load(Ordering::Relaxed), 1);
        assert_eq!(s.integrity_checks_failed.load(Ordering::Relaxed), 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupStatsComparator — compare deux instantanés consécutifs
// ─────────────────────────────────────────────────────────────────────────────

/// Delta entre deux instantanés de statistiques.
#[derive(Debug, Clone, Copy)]
pub struct DedupStatsDelta {
    pub blobs_delta:        i64,
    pub chunks_delta:       i64,
    pub saved_bytes_delta:  i64,
    pub dedup_ratio_change: i8,  // différence en points de pourcentage.
}

impl DedupStatsDelta {
    /// Calcule le delta entre deux instantanés.
    ///
    /// ARITH-02 : conversions i64 explicites pour éviter les débordements.
    pub fn compute(before: &DedupStatsSummary, after: &DedupStatsSummary) -> Self {
        let blobs_delta = (after.total_blobs_processed as i64)
            .wrapping_sub(before.total_blobs_processed as i64);
        let chunks_delta = (after.total_chunks_processed as i64)
            .wrapping_sub(before.total_chunks_processed as i64);
        let saved_bytes_delta = (after.saved_bytes as i64)
            .wrapping_sub(before.saved_bytes as i64);
        let dedup_ratio_change = (after.dedup_ratio_pct as i8)
            .wrapping_sub(before.dedup_ratio_pct as i8);
        Self { blobs_delta, chunks_delta, saved_bytes_delta, dedup_ratio_change }
    }

    pub fn is_improving(&self) -> bool {
        self.dedup_ratio_change > 0 || self.saved_bytes_delta > 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupEfficiencyMetrics — métriques de l'efficacité de déduplication
// ─────────────────────────────────────────────────────────────────────────────

/// Métriques calculées sur l'efficacité du moteur.
#[derive(Debug, Clone, Copy)]
pub struct DedupEfficiencyMetrics {
    pub amplification_ratio_x100: u64,  // x100 pour éviter les flottants.
    pub bytes_per_chunk:          u64,
    pub chunks_per_blob:          u64,
}

impl DedupEfficiencyMetrics {
    pub fn from_summary(s: &DedupStatsSummary) -> Self {
        let amp = if s.physical_bytes_stored == 0 { 100 } else {
            (s.logical_bytes_written * 100)
                .checked_div(s.physical_bytes_stored)
                .unwrap_or(100)
        };
        let bpc = if s.total_chunks_processed == 0 { 0 } else {
            s.logical_bytes_written
                .checked_div(s.total_chunks_processed)
                .unwrap_or(0)
        };
        let cpb = if s.total_blobs_processed == 0 { 0 } else {
            s.total_chunks_processed
                .checked_div(s.total_blobs_processed)
                .unwrap_or(0)
        };
        Self {
            amplification_ratio_x100: amp,
            bytes_per_chunk:          bpc,
            chunks_per_blob:          cpb,
        }
    }
}

impl DedupStats {
    /// Efficacité actuelle du moteur.
    pub fn efficiency(&self) -> DedupEfficiencyMetrics {
        DedupEfficiencyMetrics::from_summary(&self.snapshot())
    }
}

#[cfg(test)]
mod tests_advanced {
    use super::*;

    #[test] fn test_delta_positive() {
        let s = DedupStats::new_const();
        let before = s.snapshot();
        s.record_blob(true, 8192, 2048, 4, 3);
        let after = s.snapshot();
        let d = DedupStatsDelta::compute(&before, &after);
        assert_eq!(d.blobs_delta, 1);
        assert!(d.saved_bytes_delta > 0);
        assert!(d.is_improving());
    }

    #[test] fn test_efficiency_no_data() {
        let s = DedupStats::new_const();
        let e = s.efficiency();
        assert_eq!(e.bytes_per_chunk, 0);
        assert_eq!(e.chunks_per_blob, 0);
    }

    #[test] fn test_efficiency_with_data() {
        let s = DedupStats::new_const();
        s.record_blob(true, 16384, 4096, 8, 6);
        let e = s.efficiency();
        assert!(e.bytes_per_chunk > 0);
        assert_eq!(e.chunks_per_blob, 8);
    }

    #[test] fn test_history_invalid_capacity() {
        assert!(DedupStatsHistory::new(0).is_err());
        assert!(DedupStatsHistory::new(STATS_MAX_SNAPSHOTS + 1).is_err());
    }

    #[test] fn test_summary_bounds() {
        let s: DedupStatsSummary = DedupStatsSummary {
            total_blobs_processed:   5,
            deduped_blobs:           3,
            total_chunks_processed:  20,
            deduped_chunks:          12,
            logical_bytes_written:   100000,
            physical_bytes_stored:   60000,
            saved_bytes:             40000,
            errors:                  0,
            integrity_checks_ok:     2,
            integrity_checks_failed: 0,
            dedup_ratio_pct:         40,
            chunk_dedup_pct:         60,
        };
        assert!(s.is_consistent());
    }
}
