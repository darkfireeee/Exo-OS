//! Orchestrateur principal du Garbage Collector ExoFS.
//!
//! Coordonne les phases : Scan → Mark → Sweep → Finalize.
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.

use crate::fs::exofs::core::{EpochId, FsError};
use crate::fs::exofs::gc::epoch_scanner::EpochScanner;
use crate::fs::exofs::gc::gc_metrics::GcMetrics;
use crate::fs::exofs::gc::gc_state::{GcPhase, GcState, GC_STATE};
use crate::fs::exofs::gc::marker::Marker;
use crate::fs::exofs::gc::orphan_collector::OrphanCollector;
use crate::fs::exofs::gc::sweeper::Sweeper;
use crate::fs::exofs::storage::{BlobStore, SuperBlock};

/// Résultat complet d'une passe GC.
#[derive(Debug, Default)]
pub struct GcPassResult {
    pub epoch: u64,
    pub blobs_scanned: u64,
    pub blobs_marked_live: u64,
    pub blobs_swept: u64,
    pub bytes_freed: u64,
    pub orphans_collected: u64,
    pub duration_ticks: u64,
}

/// Orchestrateur GC complet.
pub struct BlobGc<'sb, 'store> {
    superblock: &'sb SuperBlock,
    store: &'store BlobStore,
    metrics: GcMetrics,
}

impl<'sb, 'store> BlobGc<'sb, 'store> {
    pub fn new(superblock: &'sb SuperBlock, store: &'store BlobStore) -> Self {
        Self {
            superblock,
            store,
            metrics: GcMetrics::new(),
        }
    }

    /// Exécute une passe GC complète pour l'epoch donnée.
    ///
    /// Séquence : Scan → Mark → Sweep (→ DeferredDeleteQueue) → Orphans → Finalize.
    pub fn run_pass(
        &mut self,
        epoch: EpochId,
        start_tick: u64,
    ) -> Result<GcPassResult, FsError> {
        // Vérifie qu'aucune passe n'est déjà active.
        if GC_STATE.is_active() {
            return Err(FsError::GcAlreadyRunning);
        }

        GC_STATE.begin_pass(epoch, start_tick);
        let mut result = GcPassResult {
            epoch: epoch.0,
            ..Default::default()
        };

        // ── Phase 1 : SCANNING ──────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Scanning);
        let scanner = EpochScanner::new(self.superblock);
        let scan_result = scanner.scan_all().map_err(|e| {
            GC_STATE.end_pass(0, 0);
            e
        })?;
        result.blobs_scanned = scan_result.index.len() as u64;

        // ── Phase 2 : MARKING ───────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Marking);
        let marker = Marker::new(self.store);
        let mark_stats = marker
            .run(&scan_result.index, &scan_result.colors)
            .map_err(|e| {
                GC_STATE.end_pass(0, 0);
                e
            })?;
        result.blobs_marked_live = mark_stats.marked_black;

        // ── Phase 3 : SWEEPING ──────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Sweeping);
        let sweep_stats = Sweeper::sweep(&scan_result.index, &scan_result.colors)
            .map_err(|e| {
                GC_STATE.end_pass(0, 0);
                e
            })?;
        result.blobs_swept   = sweep_stats.blobs_swept;
        result.bytes_freed   = sweep_stats.bytes_freed;

        // ── Phase 4 : ORPHANS ───────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Finalizing);
        let orphan_count = OrphanCollector::collect(self.store)?;
        result.orphans_collected = orphan_count;

        // ── Finalisation ────────────────────────────────────────────────────
        let end_tick = crate::arch::time::read_ticks();
        result.duration_ticks = end_tick.saturating_sub(start_tick);

        self.metrics.record_pass(&result);
        GC_STATE.end_pass(result.blobs_swept, result.bytes_freed);

        Ok(result)
    }

    /// Accès aux métriques accumulées.
    pub fn metrics(&self) -> &GcMetrics {
        &self.metrics
    }
}
