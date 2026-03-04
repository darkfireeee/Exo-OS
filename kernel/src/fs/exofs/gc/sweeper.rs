// kernel/src/fs/exofs/gc/sweeper.rs
//
// ==============================================================================
// Phase de Balayage (Sweep Phase) pour le GC tricolore
// Ring 0 . no_std . Exo-OS
//
// Ce module implemente la phase de balayage du GC tricolore :
//   Apres le marquage, les blobs restes BLANCS sont orphelins.
//   Le sweeper les collecte et les place dans la file de suppression differee.
//
// Conformite :
//   GC-01 : suppression differee via BLOB_REFCOUNT (min 2 epochs)
//   GC-07 : jamais supprimer un blob EPOCH_PINNED
//   DEAD-01 : jamais acquerir EPOCH_COMMIT_LOCK
//   RECUR-01 : traitement iteratif
//   ARITH-02 : checked_add / saturating_*
//   OOM-02 : try_reserve avant push
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================

#![allow(dead_code)]

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::epoch::epoch_pin::is_epoch_pinned;
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::tricolor::{SweepResult, TricolorWorkspace};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximal de blobs balayés par passe.
pub const MAX_SWEEP_PER_PASS: usize = 131_072;

/// Taille d'un batch de balayage (GC-05 : non bloquant).
pub const SWEEP_BATCH_SIZE: usize = 512;

// ==============================================================================
// SweeperResult — resultat complet de la phase de balayage
// ==============================================================================

/// Résultat d'une phase de balayage.
#[derive(Debug, Default, Clone)]
pub struct SweeperResult {
    /// Blobs blancs trouves apres le marquage.
    pub white_blobs_found:   u64,
    /// Blobs effectivement envoyes en suppression differee (GC-01).
    pub blobs_deferred:      u64,
    /// Blobs sautes car EPOCH_PINNED (GC-07).
    pub pinned_skipped:      u64,
    /// Blobs sautes car ref_count > 0 (encore references).
    pub refcount_skipped:    u64,
    /// Octets liberes (suppression differee programmee).
    pub bytes_deferred:      u64,
    /// Erreurs de file pleine.
    pub deferred_full_errs:  u64,
    /// Batches de balayage executes.
    pub batches_executed:    u64,
    /// Phase complete (tous les blobs blancs traites).
    pub phase_complete:      bool,
}

impl fmt::Display for SweeperResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SweeperResult[white={} deferred={} pinned={} rcskip={} bytes={} errs={} batches={}]",
            self.white_blobs_found,
            self.blobs_deferred,
            self.pinned_skipped,
            self.refcount_skipped,
            self.bytes_deferred,
            self.deferred_full_errs,
            self.batches_executed,
        )
    }
}

// ==============================================================================
// SweepConfig — configuration du sweeper
// ==============================================================================

/// Configuration de la phase de balayage.
#[derive(Debug, Clone)]
pub struct SweepConfig {
    /// Taille d'un batch de balayage.
    pub batch_size:    usize,
    /// Nombre maximal de blobs par passe.
    pub max_per_pass:  usize,
    /// Epoch courante pour la file de suppression differee.
    pub current_epoch: EpochId,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            batch_size:    SWEEP_BATCH_SIZE,
            max_per_pass:  MAX_SWEEP_PER_PASS,
            current_epoch: EpochId(0),
        }
    }
}

// ==============================================================================
// SweeperInner — etat interne
// ==============================================================================

struct SweeperInner {
    config:        SweepConfig,
    total_result:  SweeperResult,
    pass_count:    u64,
}

// ==============================================================================
// Sweeper — facade thread-safe
// ==============================================================================

/// Balayeur GC : collecte les blobs blancs apres la phase de marquage.
pub struct Sweeper {
    inner: SpinLock<SweeperInner>,
}

impl Sweeper {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(SweeperInner {
                config: SweepConfig {
                    batch_size:    SWEEP_BATCH_SIZE,
                    max_per_pass:  MAX_SWEEP_PER_PASS,
                    current_epoch: EpochId(0),
                },
                total_result: SweeperResult {
                    white_blobs_found:  0,
                    blobs_deferred:     0,
                    pinned_skipped:     0,
                    refcount_skipped:   0,
                    bytes_deferred:     0,
                    deferred_full_errs: 0,
                    batches_executed:   0,
                    phase_complete:     false,
                },
                pass_count: 0,
            }),
        }
    }

    // ── Configuration ───────────────────────────────────────────────────────

    pub fn set_config(&self, config: SweepConfig) {
        self.inner.lock().config = config;
    }

    pub fn get_config(&self) -> SweepConfig {
        self.inner.lock().config.clone()
    }

    pub fn set_epoch(&self, epoch: EpochId) {
        self.inner.lock().config.current_epoch = epoch;
    }

    // ── Phase de balayage principale ────────────────────────────────────────

    /// Lance la phase de balayage sur `workspace`.
    ///
    /// Extrait tous les blobs BLANCS restants apres le marquage,
    /// et les defere pour suppression via BLOB_REFCOUNT (GC-01).
    ///
    /// GC-07 : les blobs EPOCH_PINNED sont skipes.
    /// GC-05 : traitement par batches pour eviter le blocage.
    pub fn run_sweep_phase(
        &self,
        workspace: &mut TricolorWorkspace,
    ) -> ExofsResult<SweeperResult> {
        let (batch_size, max_per_pass, current_epoch) = {
            let g = self.inner.lock();
            (
                g.config.batch_size,
                g.config.max_per_pass,
                g.config.current_epoch,
            )
        };

        // Recuperer tous les blobs blancs du workspace.
        // collect_white() retourne Vec<(BlobId, phys_size)>.
        let white_pairs: Vec<(BlobId, u64)> = workspace.collect_white();
        // On extrait seulement les BlobIds pour le traitement par batch.
        let white_blobs: Vec<BlobId> = white_pairs.iter().map(|(b, _)| *b).collect();
        let total_white = white_blobs.len();

        let mut result = SweeperResult::default();
        result.white_blobs_found = total_white as u64;

        // Traitement par batches (RECUR-01, GC-05).
        let mut processed = 0usize;
        let mut batch_start = 0usize;

        while batch_start < total_white && processed < max_per_pass {
            let batch_end = (batch_start + batch_size).min(total_white);
            let batch = &white_blobs[batch_start..batch_end];

            let batch_r = self.sweep_batch(batch, current_epoch)?;
            result.blobs_deferred = result.blobs_deferred
                .saturating_add(batch_r.blobs_deferred);
            result.pinned_skipped = result.pinned_skipped
                .saturating_add(batch_r.pinned_skipped);
            result.refcount_skipped = result.refcount_skipped
                .saturating_add(batch_r.refcount_skipped);
            result.bytes_deferred = result.bytes_deferred
                .saturating_add(batch_r.bytes_deferred);
            result.deferred_full_errs = result.deferred_full_errs
                .saturating_add(batch_r.deferred_full_errs);
            result.batches_executed = result.batches_executed
                .saturating_add(1);

            processed = processed.saturating_add(batch.len());
            batch_start = batch_end;
        }

        if batch_start >= total_white || processed >= max_per_pass {
            result.phase_complete = batch_start >= total_white;
        } else {
            result.phase_complete = true;
        }

        // Mise a jour des metriques globales.
        GC_METRICS.add_blobs_collected(result.blobs_deferred);
        GC_METRICS.add_bytes_freed(result.bytes_deferred);
        GC_METRICS.add_deferred_flushed(result.blobs_deferred);
        GC_METRICS.add_pinned_skipped(result.pinned_skipped);

        // Enregistrement dans l'etat GC.
        GC_STATE.record_scanned(result.blobs_deferred);

        // Mise a jour des stats internes.
        {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            let t = &mut g.total_result;
            t.white_blobs_found = t.white_blobs_found
                .saturating_add(result.white_blobs_found);
            t.blobs_deferred = t.blobs_deferred
                .saturating_add(result.blobs_deferred);
            t.pinned_skipped = t.pinned_skipped
                .saturating_add(result.pinned_skipped);
            t.refcount_skipped = t.refcount_skipped
                .saturating_add(result.refcount_skipped);
            t.bytes_deferred = t.bytes_deferred
                .saturating_add(result.bytes_deferred);
            t.deferred_full_errs = t.deferred_full_errs
                .saturating_add(result.deferred_full_errs);
            t.batches_executed = t.batches_executed
                .saturating_add(result.batches_executed);
        }

        Ok(result)
    }

    /// Traite un batch de blobs blancs.
    fn sweep_batch(
        &self,
        batch:         &[BlobId],
        current_epoch: EpochId,
    ) -> ExofsResult<BatchSweepResult> {
        let mut br = BatchSweepResult::default();

        for &blob_id in batch {
            // Recuperer l'epoch de creation du blob depuis BLOB_REFCOUNT.
            let (create_epoch, phys_size) = BLOB_REFCOUNT.get_epoch_and_size(&blob_id);

            // GC-07 : ne pas supprimer si epoch pinnee.
            if is_epoch_pinned(create_epoch) {
                br.pinned_skipped = br.pinned_skipped.saturating_add(1);
                continue;
            }

            // Verifier le ref_count avant de decrements (securite).
            let rc = BLOB_REFCOUNT.get_count(&blob_id);
            if rc > 0 {
                // Encore reference — ne pas supprimer.
                br.refcount_skipped = br.refcount_skipped.saturating_add(1);
                continue;
            }

            // Decrementer le ref_count (REFCNT-01 via blob_refcount).
            // Si le count atteint zero, le blob sera mis en file differee (GC-01).
            match BLOB_REFCOUNT.dec(&blob_id, current_epoch) {
                Ok(did_defer) => {
                    if did_defer.0 == 0 {
                        br.blobs_deferred = br.blobs_deferred.saturating_add(1);
                        br.bytes_deferred = br.bytes_deferred
                            .saturating_add(phys_size);
                    }
                }
                Err(ExofsError::Resource) => {
                    // File de suppression differee pleine.
                    br.deferred_full_errs = br.deferred_full_errs.saturating_add(1);
                }
                Err(_) => {
                    br.deferred_full_errs = br.deferred_full_errs.saturating_add(1);
                }
            }
        }

        Ok(br)
    }

    // ── Duree de vie de la file differee ─────────────────────────────────────

    /// Vide la file de suppression differee pour les blobs eligibles.
    ///
    /// GC-01 : seuls les blobs avec `min_epoch <= current_epoch` sont liberes.
    pub fn flush_deferred(&self, current_epoch: EpochId) -> ExofsResult<u64> {
        let deferred_entries = BLOB_REFCOUNT.flush_deferred(current_epoch);
        let flushed = deferred_entries.len() as u64;
        GC_METRICS.add_deferred_flushed(flushed);
        Ok(flushed)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Stats cumulees.
    pub fn total_result(&self) -> SweeperResult {
        self.inner.lock().total_result.clone()
    }

    /// Nombre de passes.
    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    /// Reset des stats.
    pub fn reset_stats(&self) {
        let mut g = self.inner.lock();
        g.total_result = SweeperResult::default();
        g.pass_count = 0;
    }
}

// ==============================================================================
// BatchSweepResult — interne
// ==============================================================================

#[derive(Default)]
struct BatchSweepResult {
    blobs_deferred:      u64,
    pinned_skipped:      u64,
    refcount_skipped:    u64,
    bytes_deferred:      u64,
    deferred_full_errs:  u64,
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Balayeur GC global.
pub static SWEEPER: Sweeper = Sweeper::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::BlobId;
    use crate::fs::exofs::gc::tricolor::{BlobNode, TricolorWorkspace};

    fn bid(b: u8) -> BlobId {
        let mut a = [0u8; 32]; a[0] = b; BlobId(a)
    }

    fn white_node(b: u8) -> BlobNode {
        // ref_count = 0, non pine, epoch = 0
        BlobNode::new(bid(b), 512, 0, 0, 0, false)
    }

    #[test]
    fn test_sweep_empty_workspace() {
        let sweeper = Sweeper::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        let result = sweeper.run_sweep_phase(&mut ws).unwrap();
        assert_eq!(result.white_blobs_found, 0);
        assert!(result.phase_complete);
    }

    #[test]
    fn test_sweep_finds_white_blobs() {
        let sweeper = Sweeper::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        // Inserer 3 noeuds blancs (non grises, non noircis).
        ws.insert_node(white_node(1));
        ws.insert_node(white_node(2));
        ws.insert_node(white_node(3));
        // Tous sont blancs.
        let result = sweeper.run_sweep_phase(&mut ws).unwrap();
        assert_eq!(result.white_blobs_found, 3);
    }

    #[test]
    fn test_sweep_black_nodes_not_sweeped() {
        let sweeper = Sweeper::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        // Noeud grey -> black (marque vivant)
        ws.insert_node(white_node(1));
        ws.grey(bid(1)).unwrap();
        ws.blacken(&bid(1));
        // Noeud blanc
        ws.insert_node(white_node(2));
        let result = sweeper.run_sweep_phase(&mut ws).unwrap();
        // Seul le noeud blanc est trouve.
        assert_eq!(result.white_blobs_found, 1);
    }

    #[test]
    fn test_sweep_config() {
        let sweeper = Sweeper::new();
        sweeper.set_config(SweepConfig {
            batch_size:    16,
            max_per_pass:  100,
            current_epoch: 5,
        });
        let cfg = sweeper.get_config();
        assert_eq!(cfg.batch_size, 16);
        assert_eq!(cfg.current_epoch, 5);
    }
}
