// kernel/src/fs/exofs/gc/blob_gc.rs
//
// ==============================================================================
// Orchestrateur Principal du GC ExoFS (Blob GC)
// Ring 0 . no_std . Exo-OS
//
// Ce module orchestre la passe GC complète :
//   Phase 1 — SCAN    : lire les EpochRoots A/B/C, construire le grey set
//   Phase 2 — MARK    : propager les couleurs via la file grise
//   Phase 3 — SWEEP   : collecter les blobs blancs (orphelins)
//   Phase 4 — ORPHAN  : collecter les objets non atteignables
//   Phase 5 — INLINE  : collecter les objets inline orphelins
//   Phase 6 — FINALIZE: vider la file de suppression differee
//
// Conformite :
//   GC-01 : DeferredDeleteQueue (via BLOB_REFCOUNT / SWEEPER)
//   GC-02 : traversee des Relations (RELATION_WALKER dans MARKER)
//   GC-03 : file grise bornee (TricolorWorkspace)
//   GC-04 : try_reserve obligatoire (TricolorWorkspace)
//   GC-05 : GC toujours en background, jamais bloquant
//   GC-06 : racines GC = EpochRoots A/B/C (EPOCH_SCANNER)
//   GC-07 : jamais collecter un blob EPOCH_PINNED
//   DEAD-01 : jamais acquerir EPOCH_COMMIT_LOCK ici
//   DAG-01 : PAS d'import de arch/, ipc/, process/
// ==============================================================================

#![allow(dead_code)]

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, EpochId, ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::epoch::epoch_root::EpochRootInMemory;
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::cycle_detector::CYCLE_DETECTOR;
use crate::fs::exofs::gc::epoch_scanner::{
    BlobLookup, EmptyBlobLookup, EpochScanSnapshot, EPOCH_SCANNER,
};
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::{GcPhase, GC_STATE};
use crate::fs::exofs::gc::gc_tuning::GC_TUNER;
use crate::fs::exofs::gc::inline_gc::INLINE_GC;
use crate::fs::exofs::gc::marker::MARKER;
use crate::fs::exofs::gc::orphan_collector::ORPHAN_COLLECTOR;
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;
use crate::fs::exofs::gc::sweeper::SWEEPER;
use crate::fs::exofs::gc::tricolor::TricolorWorkspace;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximal de BlobNodes charges dans le workspace au debut d'une passe.
pub const MAX_WORKSPACE_NODES: usize = 1_000_000;

// ==============================================================================
// GcPassResult — résultat complet d'une passe GC
// ==============================================================================

/// Résultat d'une passe GC complète.
#[derive(Debug, Default, Clone)]
pub struct GcPassResult {
    // ── Phase SCAN ──────────────────────────────────────────────────────────
    pub scan_slots_valid:    u64,
    pub scan_roots_found:    u64,
    pub scan_blobs_greyed:   u64,

    // ── Phase MARK ──────────────────────────────────────────────────────────
    pub mark_nodes_black:    u64,
    pub mark_blobs_greyed:   u64,
    pub mark_pinned_skip:    u64,
    pub mark_queue_full:     u64,

    // ── Phase SWEEP ─────────────────────────────────────────────────────────
    pub sweep_white_found:   u64,
    pub sweep_deferred:      u64,
    pub sweep_bytes:         u64,
    pub sweep_pinned_skip:   u64,

    // ── Phase ORPHAN ────────────────────────────────────────────────────────
    pub orphan_objects:      u64,
    pub orphan_blobs:        u64,

    // ── Phase INLINE ────────────────────────────────────────────────────────
    pub inline_collected:    u64,
    pub inline_bytes:        u64,

    // ── Phase FINALIZE ─────────────────────────────────────────────────────
    pub finalize_flushed:    u64,

    // ── Statut global ───────────────────────────────────────────────────────
    pub cycles_detected:     u64,
    pub tick_start:          u64,
    pub tick_end:            u64,
    pub success:             bool,
    pub abort_reason:        Option<&'static str>,
}

impl GcPassResult {
    /// Duree en ticks logiques.
    pub fn tick_duration(&self) -> u64 {
        self.tick_end.saturating_sub(self.tick_start)
    }

    /// Total de blobs collectes toutes phases confondues.
    pub fn total_collected(&self) -> u64 {
        self.sweep_deferred
            .saturating_add(self.orphan_blobs)
            .saturating_add(self.inline_collected)
    }

    /// Total d'octets liberes.
    pub fn total_bytes(&self) -> u64 {
        self.sweep_bytes.saturating_add(self.inline_bytes)
    }
}

impl fmt::Display for GcPassResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcPass[ok={} ticks={} roots={} black={} sweep_def={} orph_obj={} \
             inline={} bytes={} cycles={}]",
            self.success,
            self.tick_duration(),
            self.scan_roots_found,
            self.mark_nodes_black,
            self.sweep_deferred,
            self.orphan_objects,
            self.inline_collected,
            self.total_bytes(),
            self.cycles_detected,
        )
    }
}

// ==============================================================================
// BlobGcConfig — configuration de l'orchestrateur
// ==============================================================================

/// Configuration de l'orchestrateur GC.
#[derive(Debug, Clone)]
pub struct BlobGcConfig {
    /// Activer la detection de cycles.
    pub detect_cycles:    bool,
    /// Activer la collecte d'orphelins.
    pub collect_orphans:  bool,
    /// Activer le GC inline.
    pub collect_inline:   bool,
    /// Vider la file differee en fin de passe.
    pub flush_deferred:   bool,
    /// Epoch courante (pour la file differee).
    pub current_epoch:    EpochId,
}

impl Default for BlobGcConfig {
    fn default() -> Self {
        Self {
            detect_cycles:   true,
            collect_orphans: true,
            collect_inline:  true,
            flush_deferred:  true,
            current_epoch:   EpochId(0),
        }
    }
}

// ==============================================================================
// BlobGcInner — état interne
// ==============================================================================

struct BlobGcInner {
    config:      BlobGcConfig,
    pass_count:  u64,
    last_result: Option<GcPassResult>,
}

// ==============================================================================
// BlobGc — orchestrateur principal
// ==============================================================================

/// Orchestrateur du GC ExoFS.
pub struct BlobGc {
    inner: SpinLock<BlobGcInner>,
}

impl BlobGc {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(BlobGcInner {
                config: BlobGcConfig {
                    detect_cycles:   true,
                    collect_orphans: true,
                    collect_inline:  true,
                    flush_deferred:  true,
                    current_epoch:   EpochId(0),
                },
                pass_count:  0,
                last_result: None,
            }),
        }
    }

    // ── Configuration ───────────────────────────────────────────────────────

    pub fn set_config(&self, config: BlobGcConfig) {
        self.inner.lock().config = config;
    }

    pub fn set_epoch(&self, epoch: EpochId) {
        self.inner.lock().config.current_epoch = epoch;
    }

    // ── Passe GC principale ─────────────────────────────────────────────────

    /// Lance une passe GC complete avec les EpochRoots fournis.
    ///
    /// # Arguments
    /// - `epoch_roots` : slice `[Option<&EpochRootInMemory>; 3]`
    ///   correspondant aux slots A, B, C (dans l'ordre de `EpochSlot::all()`).
    ///   `None` si le slot n'est pas disponible.
    ///
    /// Retourne un `GcPassResult` indépendamment du succès partiel.
    /// En cas d'erreur critique, la passe est abandonnée (GcPhase::Aborted).
    pub fn run_pass(
        &self,
        epoch_roots: &[Option<&EpochRootInMemory>],
    ) -> GcPassResult {
        let (config, pass_num) = {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            (g.config.clone(), g.pass_count)
        };

        let tick_start = GC_STATE.advance_tick();
        let mut result = GcPassResult {
            tick_start,
            ..GcPassResult::default()
        };

        // Initier la passe GC (Idle -> Scanning).
        if GC_STATE.begin_pass(config.current_epoch).is_err() {
            result.abort_reason = Some("begin_pass_failed");
            result.success = false;
            result.tick_end = GC_STATE.advance_tick();
            return result;
        }

        // ── PHASE 1 : SCAN ─────────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Scanning).ok();

        let scan_snapshot = match self.run_scan_phase(epoch_roots, &config) {
            Ok(snap) => {
                result.scan_slots_valid   = snap.stats.slots_valid;
                result.scan_roots_found   = snap.stats.roots_extracted;
                snap
            }
            Err(e) => {
                let _ = GC_STATE.abort_pass("scan_phase_failed");
                result.abort_reason = Some("scan_phase_failed");
                result.success = false;
                result.tick_end = GC_STATE.advance_tick();
                return result;
            }
        };

        // ── PHASE 2 : MARK ─────────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Marking).ok();

        let lookup = EmptyBlobLookup;
        let mut workspace = match self.run_mark_phase(&scan_snapshot, &lookup) {
            Ok(ws) => ws,
            Err(e) => {
                let _ = GC_STATE.abort_pass("mark_phase_oom");
                result.abort_reason = Some("mark_phase_oom");
                result.success = false;
                result.tick_end = GC_STATE.advance_tick();
                return result;
            }
        };

        // Collecter les stats de marquage.
        let mark_total = MARKER.total_result();
        result.mark_nodes_black = mark_total.nodes_blackened;
        result.mark_blobs_greyed = mark_total.blobs_greyed;
        result.mark_pinned_skip = mark_total.pinned_skipped;
        result.mark_queue_full = mark_total.queue_full_errors;

        // ── PHASE 3 : SWEEP ─────────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Sweeping).ok();
        SWEEPER.set_epoch(config.current_epoch);

        match SWEEPER.run_sweep_phase(&mut workspace) {
            Ok(sw) => {
                result.sweep_white_found = sw.white_blobs_found;
                result.sweep_deferred    = sw.blobs_deferred;
                result.sweep_bytes       = sw.bytes_deferred;
                result.sweep_pinned_skip = sw.pinned_skipped;
            }
            Err(_) => {
                // Erreur non fatale : on continue.
            }
        }

        // ── PHASE 4 : ORPHAN (optionnel) ────────────────────────────────────
        if config.collect_orphans {
            match ORPHAN_COLLECTOR.collect_orphans(&scan_snapshot, config.current_epoch) {
                Ok(orp) => {
                    result.orphan_objects = orp.objects_orphaned;
                    result.orphan_blobs   = orp.blobs_deferred;
                }
                Err(_) => {}
            }
        }

        // ── PHASE 5 : INLINE (optionnel) ────────────────────────────────────
        if config.collect_inline {
            match INLINE_GC.collect(&scan_snapshot) {
                Ok(inl) => {
                    result.inline_collected = inl.collected;
                    result.inline_bytes     = inl.bytes_freed;
                }
                Err(_) => {}
            }
        }

        // ── Détection de cycles (optionnel) ─────────────────────────────────
        if config.detect_cycles {
            let all_blobs = REFERENCE_TRACKER.all_blobs();
            match CYCLE_DETECTOR.detect_cycles(&all_blobs) {
                Ok(cycles) => {
                    result.cycles_detected = cycles.len() as u64;
                    GC_STATE.record_cycles(cycles.len() as u64);
                }
                Err(_) => {}
            }
        }

        // ── PHASE 6 : FINALIZE ──────────────────────────────────────────────
        GC_STATE.set_phase(GcPhase::Finalizing).ok();

        if config.flush_deferred {
            match SWEEPER.flush_deferred(config.current_epoch) {
                Ok(n) => {
                    result.finalize_flushed = n;
                    GC_METRICS.add_deferred_flushed(n);
                }
                Err(_) => {}
            }
        }

        // ── Clôture de la passe ─────────────────────────────────────────────
        result.tick_end = GC_STATE.advance_tick();
        result.success  = true;

        // Creer un GcPassStats a partir du resultat pour end_pass.
        let pass_stats = build_pass_stats(&result);
        GC_STATE.end_pass(pass_stats.blobs_swept, pass_stats.bytes_freed);

        // Metriques globales.
        GC_METRICS.inc_passes_completed();

        {
            let mut g = self.inner.lock();
            g.last_result = Some(result.clone());
        }

        result
    }

    // ── Phases internes ─────────────────────────────────────────────────────

    /// Phase SCAN : retourne le snapshot des EpochRoots.
    fn run_scan_phase<'a>(
        &self,
        epoch_roots: &[Option<&'a EpochRootInMemory>],
        _config:     &BlobGcConfig,
    ) -> ExofsResult<EpochScanSnapshot> {
        EPOCH_SCANNER.scan(epoch_roots)
    }

    /// Phase MARK : construit le workspace tricolore et lance le marquage.
    fn run_mark_phase<L: BlobLookup>(
        &self,
        snapshot: &EpochScanSnapshot,
        lookup:   &L,
    ) -> ExofsResult<TricolorWorkspace> {
        let mut workspace = TricolorWorkspace::new()?;

        // Charger les BlobNodes depuis REFERENCE_TRACKER dans le workspace.
        let all_blobs = REFERENCE_TRACKER.all_blobs();
        for blob_id in all_blobs.iter().take(MAX_WORKSPACE_NODES) {
            let (create_epoch, phys_size) =
                BLOB_REFCOUNT.get_epoch_and_size(blob_id);
            let rc = BLOB_REFCOUNT.get_count(blob_id);
            let node = crate::fs::exofs::gc::tricolor::BlobNode::new(
                *blob_id,
                phys_size,
                1,
                create_epoch.0,
                rc,
                false,
            );
            workspace.insert_node(node);
        }

        // Construire le grey set initial depuis le scan snapshot.
        let _greyed = EPOCH_SCANNER.build_grey_set(snapshot, &mut workspace, lookup)?;

        // Lancer le marquage.
        MARKER.run_mark_phase(&mut workspace)?;

        Ok(workspace)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    pub fn last_result(&self) -> Option<GcPassResult> {
        self.inner.lock().last_result.clone()
    }

    pub fn get_config(&self) -> BlobGcConfig {
        self.inner.lock().config.clone()
    }
}

// ==============================================================================
// Helper — construire GcPassStats depuis GcPassResult
// ==============================================================================

fn build_pass_stats(
    r: &GcPassResult,
) -> crate::fs::exofs::gc::gc_state::GcPassStats {
    crate::fs::exofs::gc::gc_state::GcPassStats {
        blobs_scanned:     r.scan_roots_found,
        blobs_marked_live: r.mark_nodes_black,
        blobs_swept:       r.sweep_white_found,
        bytes_freed:       r.total_bytes(),
        orphans_collected: r.orphan_objects,
        inline_gc_count:   r.inline_collected,
        cycles_detected:   r.cycles_detected,
        start_tick:        r.tick_duration(),
        end_tick:          r.tick_duration(),
        completed:         r.success,
        abort_reason:      r.abort_reason,
        epoch:             0,
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Orchestrateur GC global.
pub static BLOB_GC: BlobGc = BlobGc::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::gc::blob_gc::{BlobGc, BlobGcConfig};

    #[test]
    fn test_pass_empty_roots() {
        let gc = BlobGc::new();
        // Passe avec aucune EpochRoot -> doit reussir sans crasher.
        let result = gc.run_pass(&[None, None, None]);
        // Le scan reussit même avec des slots vides.
        // begin_pass peut échouer si GC_STATE est déjà actif dans un autre test,
        // donc on vérifie juste que la fonction retourne.
        let _ = result;
    }

    #[test]
    fn test_pass_count_increments() {
        let gc = BlobGc::new();
        let _ = gc.run_pass(&[None, None, None]);
        // pass_count est incrémenté avant begin_pass.
        assert_eq!(gc.pass_count(), 1);
    }

    #[test]
    fn test_config_default() {
        let gc = BlobGc::new();
        let cfg = gc.get_config();
        assert!(cfg.detect_cycles);
        assert!(cfg.collect_orphans);
        assert!(cfg.collect_inline);
        assert!(cfg.flush_deferred);
    }

    #[test]
    fn test_set_epoch() {
        let gc = BlobGc::new();
        gc.set_epoch(EpochId(42));
        assert_eq!(gc.get_config().current_epoch, EpochId(42));
    }

    #[test]
    fn test_pass_result_display() {
        let r = GcPassResult {
            success:         true,
            scan_roots_found: 10,
            mark_nodes_black: 8,
            sweep_deferred:   2,
            orphan_objects:   1,
            inline_collected: 1,
            sweep_bytes:      4096,
            inline_bytes:     256,
            cycles_detected:  0,
            tick_start:       100,
            tick_end:         200,
            ..GcPassResult::default()
        };
        assert_eq!(r.tick_duration(), 100);
        assert_eq!(r.total_collected(), 3);
        assert_eq!(r.total_bytes(), 4352);
        let s = alloc::format!("{}", r);
        assert!(s.contains("ok=true"));
    }
}
