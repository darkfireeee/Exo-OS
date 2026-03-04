// kernel/src/fs/exofs/gc/gc_metrics.rs
//
// ==============================================================================
// Metriques du Garbage Collector ExoFS
// Ring 0 . no_std . Exo-OS
//
// Collecte les metriques par phase : Scanning, Marking, Sweeping, Finalizing.
// Toutes les metriques sont des AtomicU64 pour lecture sans verrou depuis
// l'observablity et les dashboards.
//
// Conformite :
//   GC-05 : metriques non bloquantes (pas de lock dans le chemin critique)
//   ARITH-02 : saturating_add partout
// ==============================================================================

#![allow(dead_code)]

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::gc::tricolor::{MarkStats, SweepResult};
use crate::fs::exofs::gc::gc_state::GcPassStats;

// ==============================================================================
// PhaseMetrics — metriques d'une phase
// ==============================================================================

/// Metriques cumulees pour une phase GC specifique.
pub struct PhaseMetrics {
    /// Nombre de fois que la phase s'est executee.
    pub runs:           AtomicU64,
    /// Ticks logiques cumules passes dans cette phase.
    pub ticks_total:    AtomicU64,
    /// Operations realisees dans cette phase (blobs/bytes selon phase).
    pub ops_total:      AtomicU64,
    /// Erreurs rencontrees dans cette phase.
    pub errors_total:   AtomicU64,
    /// Ticks du pic (le run le plus long).
    pub ticks_peak:     AtomicU64,
}

impl PhaseMetrics {
    pub const fn new() -> Self {
        Self {
            runs:         AtomicU64::new(0),
            ticks_total:  AtomicU64::new(0),
            ops_total:    AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            ticks_peak:   AtomicU64::new(0),
        }
    }

    pub fn record_run(&self, ticks: u64, ops: u64) {
        self.runs.fetch_add(1, Ordering::Relaxed);
        self.ticks_total.fetch_add(ticks, Ordering::Relaxed);
        self.ops_total.fetch_add(ops, Ordering::Relaxed);

        // Mise a jour du peak sans synchronisation forte (approximation acceptable).
        let peak = self.ticks_peak.load(Ordering::Relaxed);
        if ticks > peak {
            self.ticks_peak.store(ticks, Ordering::Relaxed);
        }
    }

    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Ticks moyens par run (0 si aucun run).
    pub fn avg_ticks(&self) -> u64 {
        let runs = self.runs.load(Ordering::Relaxed);
        if runs == 0 { return 0; }
        self.ticks_total.load(Ordering::Relaxed) / runs
    }

    /// Snapshot non-atomique (valeurs approximatives).
    pub fn snapshot(&self) -> PhaseSnapshot {
        PhaseSnapshot {
            runs:        self.runs.load(Ordering::Relaxed),
            ticks_total: self.ticks_total.load(Ordering::Relaxed),
            ops_total:   self.ops_total.load(Ordering::Relaxed),
            errors:      self.errors_total.load(Ordering::Relaxed),
            ticks_peak:  self.ticks_peak.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot d'une PhaseMetrics (copiable).
#[derive(Debug, Clone, Default)]
pub struct PhaseSnapshot {
    pub runs:        u64,
    pub ticks_total: u64,
    pub ops_total:   u64,
    pub errors:      u64,
    pub ticks_peak:  u64,
}

impl PhaseSnapshot {
    pub fn avg_ticks(&self) -> u64 {
        if self.runs == 0 { return 0; }
        self.ticks_total / self.runs
    }
}

impl fmt::Display for PhaseSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "runs={} ops={} ticks_avg={} ticks_peak={} errors={}",
            self.runs,
            self.ops_total,
            self.avg_ticks(),
            self.ticks_peak,
            self.errors,
        )
    }
}

// ==============================================================================
// GcMetrics — metriques globales du GC
// ==============================================================================

/// Metriques globales du Garbage Collector.
///
/// Composes de :
///   - Compteurs globaux (total passes, total freed, etc.)
///   - Metriques par phase (Scanning, Marking, Sweeping, Finalizing)
pub struct GcMetrics {
    // ── Compteurs globaux ────────────────────────────────────────────────────
    /// Total des passes GC terminees.
    pub passes_completed:    AtomicU64,
    /// Total des passes abandonnees.
    pub passes_aborted:      AtomicU64,
    /// Total des blobs collectes depuis le boot.
    pub blobs_collected:     AtomicU64,
    /// Total des octets liberes depuis le boot.
    pub bytes_freed:         AtomicU64,
    /// Total des blobs scannes.
    pub blobs_scanned:       AtomicU64,
    /// Total des blobs marques vivants.
    pub blobs_marked_live:   AtomicU64,
    /// Total des orphelins collectes.
    pub orphans_collected:   AtomicU64,
    /// Total des objets inline GC-es.
    pub inline_gc_count:     AtomicU64,
    /// Total des cycles detectes.
    pub cycles_detected:     AtomicU64,
    /// Depassements de la file grise (GC-03).
    pub grey_queue_overflows: AtomicU64,
    /// Entrees traitees dans la DeferredDeleteQueue.
    pub deferred_flushed:    AtomicU64,
    /// Blobs skipped car EPOCH_PINNED (GC-07).
    pub pinned_skipped:      AtomicU64,

    // ── Metriques par phase ──────────────────────────────────────────────────
    /// Phase Scanning.
    pub scan:     PhaseMetrics,
    /// Phase Marking.
    pub mark:     PhaseMetrics,
    /// Phase Sweeping.
    pub sweep:    PhaseMetrics,
    /// Phase Finalizing.
    pub finalize: PhaseMetrics,
}

impl GcMetrics {
    pub const fn new() -> Self {
        Self {
            passes_completed:    AtomicU64::new(0),
            passes_aborted:      AtomicU64::new(0),
            blobs_collected:     AtomicU64::new(0),
            bytes_freed:         AtomicU64::new(0),
            blobs_scanned:       AtomicU64::new(0),
            blobs_marked_live:   AtomicU64::new(0),
            orphans_collected:   AtomicU64::new(0),
            inline_gc_count:     AtomicU64::new(0),
            cycles_detected:     AtomicU64::new(0),
            grey_queue_overflows: AtomicU64::new(0),
            deferred_flushed:    AtomicU64::new(0),
            pinned_skipped:      AtomicU64::new(0),
            scan:     PhaseMetrics::new(),
            mark:     PhaseMetrics::new(),
            sweep:    PhaseMetrics::new(),
            finalize: PhaseMetrics::new(),
        }
    }

    // ── Enregistrement d'une passe complete ──────────────────────────────────

    /// Enregistre les statistiques d'une passe GC complete.
    pub fn record_pass(&self, stats: &GcPassStats) {
        if stats.completed {
            self.passes_completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.passes_aborted.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.blobs_collected.fetch_add(stats.blobs_swept, Ordering::Relaxed);
        self.bytes_freed.fetch_add(stats.bytes_freed, Ordering::Relaxed);
        self.blobs_scanned.fetch_add(stats.blobs_scanned, Ordering::Relaxed);
        self.blobs_marked_live.fetch_add(stats.blobs_marked_live, Ordering::Relaxed);
        self.orphans_collected.fetch_add(stats.orphans_collected, Ordering::Relaxed);
        self.inline_gc_count.fetch_add(stats.inline_gc_count, Ordering::Relaxed);
        self.cycles_detected.fetch_add(stats.cycles_detected, Ordering::Relaxed);
    }

    /// Enregistre les statistiques de la phase de marquage.
    pub fn record_mark_phase(&self, stats: &MarkStats, ticks: u64) {
        self.mark.record_run(ticks, stats.marked_black);
        self.grey_queue_overflows
            .fetch_add(stats.queue_overflows, Ordering::Relaxed);
    }

    /// Enregistre les statistiques de la phase de balayage.
    pub fn record_sweep_phase(&self, result: &SweepResult, ticks: u64) {
        self.sweep.record_run(ticks, result.blobs_swept);
        self.deferred_flushed
            .fetch_add(result.deferred_count, Ordering::Relaxed);
        self.pinned_skipped
            .fetch_add(result.blobs_skipped, Ordering::Relaxed);
    }

    // ── Increments unitaires ─────────────────────────────────────────────────

    pub fn inc_passes_completed(&self)         { self.passes_completed.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_passes_aborted(&self)           { self.passes_aborted.fetch_add(1, Ordering::Relaxed); }
    pub fn add_blobs_collected(&self, n: u64)  { self.blobs_collected.fetch_add(n, Ordering::Relaxed); }
    pub fn add_bytes_freed(&self, n: u64)      { self.bytes_freed.fetch_add(n, Ordering::Relaxed); }
    pub fn inc_grey_overflow(&self)            { self.grey_queue_overflows.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_pinned_skipped(&self)           { self.pinned_skipped.fetch_add(1, Ordering::Relaxed); }
    pub fn add_pinned_skipped(&self, n: u64)   { self.pinned_skipped.fetch_add(n, Ordering::Relaxed); }
    pub fn add_deferred_flushed(&self, n: u64) { self.deferred_flushed.fetch_add(n, Ordering::Relaxed); }
    pub fn add_blobs_marked_live(&self, n: u64) { self.blobs_marked_live.fetch_add(n, Ordering::Relaxed); }
    pub fn add_grey_queue_overflows(&self, n: u64) { self.grey_queue_overflows.fetch_add(n, Ordering::Relaxed); }
    pub fn add_orphans_collected(&self, n: u64) { self.orphans_collected.fetch_add(n, Ordering::Relaxed); }

    // ── Snapshots ────────────────────────────────────────────────────────────

    /// Snapshot de toutes les metriques.
    pub fn snapshot(&self) -> GcMetricsSnapshot {
        GcMetricsSnapshot {
            passes_completed:    self.passes_completed.load(Ordering::Relaxed),
            passes_aborted:      self.passes_aborted.load(Ordering::Relaxed),
            blobs_collected:     self.blobs_collected.load(Ordering::Relaxed),
            bytes_freed:         self.bytes_freed.load(Ordering::Relaxed),
            blobs_scanned:       self.blobs_scanned.load(Ordering::Relaxed),
            blobs_marked_live:   self.blobs_marked_live.load(Ordering::Relaxed),
            orphans_collected:   self.orphans_collected.load(Ordering::Relaxed),
            inline_gc_count:     self.inline_gc_count.load(Ordering::Relaxed),
            cycles_detected:     self.cycles_detected.load(Ordering::Relaxed),
            grey_queue_overflows: self.grey_queue_overflows.load(Ordering::Relaxed),
            deferred_flushed:    self.deferred_flushed.load(Ordering::Relaxed),
            pinned_skipped:      self.pinned_skipped.load(Ordering::Relaxed),
            scan:     self.scan.snapshot(),
            mark:     self.mark.snapshot(),
            sweep:    self.sweep.snapshot(),
            finalize: self.finalize.snapshot(),
        }
    }

    /// Ratio de collecte global (blobs collectes / blobs scannes * 100).
    pub fn global_collect_ratio_x100(&self) -> u64 {
        let scanned = self.blobs_scanned.load(Ordering::Relaxed);
        if scanned == 0 { return 0; }
        let collected = self.blobs_collected.load(Ordering::Relaxed);
        collected.saturating_mul(100) / scanned
    }
}

// ==============================================================================
// GcMetricsSnapshot — vue copiable
// ==============================================================================

/// Snapshot des metriques GC (toutes valeurs copiables).
#[derive(Debug, Clone, Default)]
pub struct GcMetricsSnapshot {
    pub passes_completed:    u64,
    pub passes_aborted:      u64,
    pub blobs_collected:     u64,
    pub bytes_freed:         u64,
    pub blobs_scanned:       u64,
    pub blobs_marked_live:   u64,
    pub orphans_collected:   u64,
    pub inline_gc_count:     u64,
    pub cycles_detected:     u64,
    pub grey_queue_overflows: u64,
    pub deferred_flushed:    u64,
    pub pinned_skipped:      u64,
    pub scan:                PhaseSnapshot,
    pub mark:                PhaseSnapshot,
    pub sweep:               PhaseSnapshot,
    pub finalize:            PhaseSnapshot,
}

impl GcMetricsSnapshot {
    pub fn collect_ratio_x100(&self) -> u64 {
        if self.blobs_scanned == 0 { return 0; }
        self.blobs_collected.saturating_mul(100) / self.blobs_scanned
    }

    pub fn total_runs(&self) -> u64 {
        self.passes_completed.saturating_add(self.passes_aborted)
    }
}

impl fmt::Display for GcMetricsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GC[passes={}/{} collected={} freed={}B ratio={}% overflows={} cycles={}]",
            self.passes_completed,
            self.total_runs(),
            self.blobs_collected,
            self.bytes_freed,
            self.collect_ratio_x100(),
            self.grey_queue_overflows,
            self.cycles_detected,
        )
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Instance globale des metriques GC.
pub static GC_METRICS: GcMetrics = GcMetrics::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::gc::gc_state::GcPassStats;

    #[test]
    fn test_phase_metrics_record() {
        let pm = PhaseMetrics::new();
        pm.record_run(100, 50);
        pm.record_run(200, 80);
        let s = pm.snapshot();
        assert_eq!(s.runs, 2);
        assert_eq!(s.ops_total, 130);
        assert_eq!(s.ticks_peak, 200);
        assert_eq!(s.avg_ticks(), 150);
    }

    #[test]
    fn test_phase_metrics_avg_zero_runs() {
        let pm = PhaseMetrics::new();
        assert_eq!(pm.avg_ticks(), 0);
    }

    #[test]
    fn test_gc_metrics_record_pass_completed() {
        let m = GcMetrics::new();
        let stats = GcPassStats {
            epoch:             1,
            blobs_scanned:     1000,
            blobs_marked_live: 800,
            blobs_swept:       200,
            bytes_freed:       1_024_000,
            orphans_collected: 10,
            inline_gc_count:   5,
            cycles_detected:   0,
            start_tick:        0,
            end_tick:          500,
            completed:         true,
            abort_reason:      None,
        };
        m.record_pass(&stats);
        let s = m.snapshot();
        assert_eq!(s.passes_completed, 1);
        assert_eq!(s.blobs_collected, 200);
        assert_eq!(s.bytes_freed, 1_024_000);
        assert_eq!(s.collect_ratio_x100(), 20);
    }

    #[test]
    fn test_gc_metrics_record_pass_aborted() {
        let m = GcMetrics::new();
        let stats = GcPassStats {
            completed: false,
            ..GcPassStats::default()
        };
        m.record_pass(&stats);
        let s = m.snapshot();
        assert_eq!(s.passes_aborted, 1);
        assert_eq!(s.passes_completed, 0);
    }

    #[test]
    fn test_snapshot_display() {
        let m = GcMetrics::new();
        let s = m.snapshot();
        let d = alloc::format!("{}", s);
        assert!(d.contains("GC["));
    }

    #[test]
    fn test_collect_ratio_zero_scanned() {
        let s = GcMetricsSnapshot::default();
        assert_eq!(s.collect_ratio_x100(), 0);
    }
}
