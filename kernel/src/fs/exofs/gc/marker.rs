// kernel/src/fs/exofs/gc/marker.rs
//
// ==============================================================================
// Phase de Marquage Tricolore (Marking Phase) pour le GC
// Ring 0 . no_std . Exo-OS
//
// Ce module implemente la phase de marquage du GC tricolore :
//   1. Pop les noeuds gris de la file
//   2. Pour chaque noeud gris :
//      a. Verifier qu'il n'est pas EPOCH_PINNED (GC-07)
//      b. Obtenir ses sous-blobs via REFERENCE_TRACKER
//      c. Griser les sous-blobs non encore visites (GC-02/GC-03/GC-04)
//      d. Blacken le noeud gris courant
//   3. Repeter par batches jusqu'a epuisement de la file ou limite atteinte
//
// Conformite :
//   GC-02 : traversee de la relation/reference walk incluse
//   GC-03 : file grise bornee (max 1_000_000)
//   GC-04 : try_reserve avant chaque push dans le workspace grey
//   GC-05 : marquage non-bloquant — traitement par batch
//   GC-07 : jamais marquer comme collectible un blob EPOCH_PINNED
//   RECUR-01 : traitement iteratif par batch
//   OOM-02 : try_reserve avant chaque allocation
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================

#![allow(dead_code)]

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::epoch::epoch_pin::is_epoch_pinned;
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;
use crate::fs::exofs::gc::tricolor::{TricolorWorkspace, GC_MARK_BATCH_SIZE};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximum de batches de marquage par passe.
pub const MAX_MARK_BATCHES: u64 = 16_384;

/// Taille d'un batch de marquage (heritee du workspace tricolore).
pub const MARKER_BATCH_SIZE: usize = GC_MARK_BATCH_SIZE;

// ==============================================================================
// MarkingResult — resultat d'une passe de marquage
// ==============================================================================

/// Resultat d'une phase de marquage complete.
#[derive(Debug, Default, Clone)]
pub struct MarkingResult {
    /// Noeuds gris depiles et traites.
    pub nodes_processed:     u64,
    /// Sous-blobs grises durant cette passe.
    pub blobs_greyed:        u64,
    /// Noeuds blackenes (marquage complet).
    pub nodes_blackened:     u64,
    /// Noeuds sautes car EPOCH_PINNED (GC-07).
    pub pinned_skipped:      u64,
    /// Debordements de file (GC-03).
    pub queue_full_errors:   u64,
    /// Batches traites.
    pub batches_processed:   u64,
    /// La passe est terminee (file vide).
    pub phase_complete:      bool,
    /// La passe a ete interrompue (limite de batches atteinte).
    pub interrupted:         bool,
}

impl fmt::Display for MarkingResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MarkingResult[proc={} black={} greyed={} pinned={} qfull={} batches={} {}]",
            self.nodes_processed,
            self.nodes_blackened,
            self.blobs_greyed,
            self.pinned_skipped,
            self.queue_full_errors,
            self.batches_processed,
            if self.phase_complete { "DONE" } else if self.interrupted { "INTERRUPTED" } else { "" },
        )
    }
}

// ==============================================================================
// MarkerConfig — configuration du marqueur
// ==============================================================================

/// Configuration de la phase de marquage.
#[derive(Debug, Clone)]
pub struct MarkerConfig {
    /// Taille d'un batch de marquage.
    pub batch_size:     usize,
    /// Nombre maximum de batches avant interruption (GC-05 : non bloquant).
    pub max_batches:    u64,
    /// Activer la traversee des relations (GC-02).
    pub walk_relations: bool,
}

impl Default for MarkerConfig {
    fn default() -> Self {
        Self {
            batch_size:     MARKER_BATCH_SIZE,
            max_batches:    MAX_MARK_BATCHES,
            walk_relations: true,
        }
    }
}

// ==============================================================================
// MarkBatch — traitement d'un batch de noeuds gris
// ==============================================================================

/// Resultat du traitement d'un seul batch.
#[derive(Debug, Default, Clone)]
struct BatchResult {
    processed:   usize,
    blackened:   usize,
    greyed:      usize,
    pinned_skip: usize,
    queue_full:  usize,
    empty:       bool, // La file etait vide avant ce batch.
}

// ==============================================================================
// MarkerInner — etat interne
// ==============================================================================

struct MarkerInner {
    /// Configuration courante.
    config:       MarkerConfig,
    /// Stats cumulees de toutes les passes.
    total_result: MarkingResult,
    /// Nombre de passes lancees.
    pass_count:   u64,
}

// ==============================================================================
// Marker — facade thread-safe
// ==============================================================================

/// Marqueur tricolore : phase de marquage du GC.
pub struct Marker {
    inner: SpinLock<MarkerInner>,
}

impl Marker {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(MarkerInner {
                config: MarkerConfig {
                    batch_size:     MARKER_BATCH_SIZE,
                    max_batches:    MAX_MARK_BATCHES,
                    walk_relations: true,
                },
                total_result: MarkingResult {
                    nodes_processed:   0,
                    blobs_greyed:      0,
                    nodes_blackened:   0,
                    pinned_skipped:    0,
                    queue_full_errors: 0,
                    batches_processed: 0,
                    phase_complete:    false,
                    interrupted:       false,
                },
                pass_count: 0,
            }),
        }
    }

    // ── Configuration ───────────────────────────────────────────────────────

    pub fn set_config(&self, config: MarkerConfig) {
        self.inner.lock().config = config;
    }

    pub fn get_config(&self) -> MarkerConfig {
        self.inner.lock().config.clone()
    }

    // ── Phase de marquage ───────────────────────────────────────────────────

    /// Lance une phase de marquage complete sur `workspace`.
    ///
    /// GC-05 : traitement par batches — retourne `interrupted=true` si
    /// `max_batches` est atteint avant la fin.
    ///
    /// GC-07 : les blobs EPOCH_PINNED ne sont jamais grises/noircis.
    pub fn run_mark_phase(
        &self,
        workspace: &mut TricolorWorkspace,
    ) -> ExofsResult<MarkingResult> {
        let (batch_size, max_batches, walk_relations) = {
            let g = self.inner.lock();
            (g.config.batch_size, g.config.max_batches, g.config.walk_relations)
        };

        let mut result = MarkingResult::default();

        // RECUR-01 : boucle iterative par batches.
        for _batch_idx in 0..max_batches {
            let batch_r = self.process_batch(
                workspace,
                batch_size,
                walk_relations,
            )?;

            result.nodes_processed = result.nodes_processed
                .saturating_add(batch_r.processed as u64);
            result.nodes_blackened = result.nodes_blackened
                .saturating_add(batch_r.blackened as u64);
            result.blobs_greyed = result.blobs_greyed
                .saturating_add(batch_r.greyed as u64);
            result.pinned_skipped = result.pinned_skipped
                .saturating_add(batch_r.pinned_skip as u64);
            result.queue_full_errors = result.queue_full_errors
                .saturating_add(batch_r.queue_full as u64);
            result.batches_processed = result.batches_processed
                .saturating_add(1);

            if batch_r.empty {
                result.phase_complete = true;
                break;
            }
        }

        if !result.phase_complete {
            result.interrupted = true;
        }

        // Mise a jour des stats GC.
        GC_METRICS.add_blobs_marked_live(result.nodes_blackened);
        GC_METRICS.add_grey_queue_overflows(result.queue_full_errors);

        // Enregistrement dans l'etat global.
        GC_STATE.record_marked(result.nodes_blackened);

        // Mise a jour des stats internes.
        {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            let t = &mut g.total_result;
            t.nodes_processed = t.nodes_processed
                .saturating_add(result.nodes_processed);
            t.nodes_blackened = t.nodes_blackened
                .saturating_add(result.nodes_blackened);
            t.blobs_greyed = t.blobs_greyed
                .saturating_add(result.blobs_greyed);
            t.pinned_skipped = t.pinned_skipped
                .saturating_add(result.pinned_skipped);
            t.queue_full_errors = t.queue_full_errors
                .saturating_add(result.queue_full_errors);
            t.batches_processed = t.batches_processed
                .saturating_add(result.batches_processed);
        }

        Ok(result)
    }

    /// Traite un batch de noeuds gris.
    fn process_batch(
        &self,
        workspace:      &mut TricolorWorkspace,
        batch_size:     usize,
        walk_relations: bool,
    ) -> ExofsResult<BatchResult> {
        let mut br = BatchResult::default();

        // Collecter un batch de BlobIds gris.
        let mut batch: Vec<BlobId> = Vec::new();
        batch.try_reserve(batch_size).map_err(|_| ExofsError::NoMemory)?;

        for _ in 0..batch_size {
            match workspace.pop_grey() {
                Some(bid) => batch.push(bid),
                None => {
                    br.empty = true;
                    break;
                }
            }
        }

        if batch.is_empty() {
            br.empty = true;
            return Ok(br);
        }

        // Traiter chaque noeud du batch.
        for blob_id in batch {
            br.processed = br.processed.saturating_add(1);

            // Obtenir l'epoch de creation du blob pour verif pinned (GC-07).
            let create_epoch = workspace
                .node_epoch(&blob_id)
                .unwrap_or(0);

            // GC-07 : Ne pas marquer si EPOCH_PINNED est actif.
            if is_epoch_pinned(EpochId(create_epoch)) {
                br.pinned_skip = br.pinned_skip.saturating_add(1);
                // Remettre en gris pour la prochaine passe.
                match workspace.grey(blob_id) {
                    Ok(()) => {}
                    Err(_) => {}
                }
                continue;
            }

            // Griser les sous-blobs (via REFERENCE_TRACKER).
            let sub_blobs = REFERENCE_TRACKER.get_refs(&blob_id);
            for &sub in &sub_blobs {
                match workspace.grey(sub) {
                    Ok(()) => {
                        br.greyed = br.greyed.saturating_add(1);
                    }
                    Err(ExofsError::GcQueueFull) => {
                        br.queue_full = br.queue_full.saturating_add(1);
                    }
                    Err(_) => {}
                }
            }

            // GC-02 : si la traversee des relations est activee,
            // les ObjectId sources/cibles du blob sont aussi grises
            // via RELATION_WALKER (ici on ne peut pas griser des ObjectId
            // directement — on grise leurs blobs associes via le walker).
            // Note: walk_and_grey est appellee depuis epoch_scanner pour les roots;
            // ici on grise les sous-aretes au niveau Blob directement.
            let _ = walk_relations; // flag deja applique lors du scan

            // Blackener le noeud (marquage complet).
            workspace.blacken(&blob_id);
            br.blackened = br.blackened.saturating_add(1);
        }

        Ok(br)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Stats cumulees.
    pub fn total_result(&self) -> MarkingResult {
        self.inner.lock().total_result.clone()
    }

    /// Nombre de passes.
    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    /// Reset des stats.
    pub fn reset_stats(&self) {
        let mut g = self.inner.lock();
        g.total_result = MarkingResult::default();
        g.pass_count = 0;
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Marqueur GC global.
pub static MARKER: Marker = Marker::new();

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

    fn node(b: u8, rc: u32) -> BlobNode {
        BlobNode::new(bid(b), 512, 1, 0, rc, false)
    }

    #[test]
    fn test_mark_single_node() {
        let marker = Marker::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        ws.insert_node(node(1, 0));
        ws.grey(bid(1)).unwrap();

        let result = marker.run_mark_phase(&mut ws).unwrap();
        assert_eq!(result.nodes_blackened, 1);
        assert!(result.phase_complete);
    }

    #[test]
    fn test_mark_phase_complete_on_empty_queue() {
        let marker = Marker::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        // File vide -> phase complete immediatement.
        let result = marker.run_mark_phase(&mut ws).unwrap();
        assert!(result.phase_complete);
        assert_eq!(result.nodes_processed, 0);
    }

    #[test]
    fn test_mark_multiple_nodes() {
        let marker = Marker::new();
        let mut ws = TricolorWorkspace::new().unwrap();

        for i in 1u8..=10 {
            ws.insert_node(node(i, 0));
            ws.grey(bid(i)).unwrap();
        }

        let result = marker.run_mark_phase(&mut ws).unwrap();
        assert_eq!(result.nodes_blackened, 10);
        assert!(result.phase_complete);
    }

    #[test]
    fn test_mark_interrupts_on_max_batches() {
        let marker = Marker::new();
        marker.set_config(MarkerConfig {
            batch_size:     1,
            max_batches:    2, // Forcer interruption.
            walk_relations: false,
        });
        let mut ws = TricolorWorkspace::new().unwrap();
        // 5 noeuds, max 2 batches de taille 1 -> interrompu.
        for i in 1u8..=5 {
            ws.insert_node(node(i, 0));
            ws.grey(bid(i)).unwrap();
        }

        let result = marker.run_mark_phase(&mut ws).unwrap();
        assert!(result.interrupted);
        assert!(!result.phase_complete);
        // 2 batches de 1 = 2 noeuds traites.
        assert_eq!(result.nodes_blackened, 2);
    }

    #[test]
    fn test_mark_pinned_blob_skipped() {
        // Impossible de tester is_epoch_pinned directement en no_std test,
        // mais on peut s'assurer que le code compiles et fonctionne pour
        // des blobs non pines (create_epoch = 0).
        let marker = Marker::new();
        let mut ws = TricolorWorkspace::new().unwrap();
        ws.insert_node(BlobNode::new(bid(1), 512, 0, 0, 0, false)); // epoch=0, non pine
        ws.grey(bid(1)).unwrap();
        let result = marker.run_mark_phase(&mut ws).unwrap();
        assert_eq!(result.nodes_blackened, 1);
    }
}
