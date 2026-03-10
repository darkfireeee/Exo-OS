// kernel/src/fs/exofs/gc/orphan_collector.rs
//
// ==============================================================================
// Collecteur d'Objets et Blobs Orphelins ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ce module identifie les objets et blobs qui ne sont plus accessibles
// depuis aucune EpochRoot valide (slots A/B/C).
//
// Algorithm :
//   1. Parcourir les objets connus dans REFERENCE_TRACKER
//   2. Marquer comme atteignables tous les objets presents dans au moins
//      un EpochRoot valide (reachable set = epoch_scanner snapshot)
//   3. Les objets non marques = orphelins
//   4. Resoudre leurs BlobIds via REFERENCE_TRACKER
//   5. Envoyer les blobs orphelins en suppression differee
//
// Conformite :
//   GC-01 : suppression differee (via BLOB_REFCOUNT)
//   GC-07 : jamais supprimer un blob EPOCH_PINNED
//   RECUR-01 : traversee iterative BFS
//   OOM-02 : try_reserve avant push
//   ARITH-02 : saturating_*
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================


use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, EpochId, ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::epoch::epoch_pin::is_epoch_pinned;
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::epoch_scanner::EpochScanSnapshot;
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximum d'orphelins traites par passe.
pub const MAX_ORPHANS_PER_PASS: usize = 65536;

/// Taille d'un batch de traitement d'orphelins.
pub const ORPHAN_BATCH_SIZE: usize = 256;

// ==============================================================================
// OrphanResult — résultat de la collecte d'orphelins
// ==============================================================================

/// Résultat d'une passe de collecte d'orphelins.
#[derive(Debug, Default, Clone)]
pub struct OrphanResult {
    /// Objets totaux analyses.
    pub objects_analyzed:    u64,
    /// Objets atteignables depuis les EpochRoots.
    pub objects_reachable:   u64,
    /// Objets orphelins detectes.
    pub objects_orphaned:    u64,
    /// Blobs orphelins detectes.
    pub blobs_orphaned:      u64,
    /// Blobs envoyes en suppression differee.
    pub blobs_deferred:      u64,
    /// Blobs sautes car EPOCH_PINNED (GC-07).
    pub pinned_skipped:      u64,
    /// Octets liberes (differes).
    pub bytes_deferred:      u64,
    /// Erreurs de file pleine.
    pub deferred_full_errs:  u64,
    /// Phase complete.
    pub phase_complete:      bool,
}

impl fmt::Display for OrphanResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OrphanResult[analyzed={} reach={} orph_obj={} orph_blob={} defer={} bytes={} pinned={}]",
            self.objects_analyzed,
            self.objects_reachable,
            self.objects_orphaned,
            self.blobs_orphaned,
            self.blobs_deferred,
            self.bytes_deferred,
            self.pinned_skipped,
        )
    }
}

// ==============================================================================
// OrphanCollectorInner
// ==============================================================================

struct OrphanCollectorInner {
    total_result: OrphanResult,
    pass_count:   u64,
}

// ==============================================================================
// OrphanCollector — facade thread-safe
// ==============================================================================

/// Collecteur d'objets et blobs orphelins.
pub struct OrphanCollector {
    inner: SpinLock<OrphanCollectorInner>,
}

impl OrphanCollector {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(OrphanCollectorInner {
                total_result: OrphanResult {
                    objects_analyzed:   0,
                    objects_reachable:  0,
                    objects_orphaned:   0,
                    blobs_orphaned:     0,
                    blobs_deferred:     0,
                    pinned_skipped:     0,
                    bytes_deferred:     0,
                    deferred_full_errs: 0,
                    phase_complete:     false,
                },
                pass_count: 0,
            }),
        }
    }

    // ── Phase principale ─────────────────────────────────────────────────────

    /// Collecte les objets et blobs orphelins en utilisant le snapshot
    /// de scan des EpochRoots comme ensemble de racines atteignables.
    ///
    /// # Arguments
    /// - `scan_snapshot` : le snapshot produit par EpochScanner::scan()
    /// - `current_epoch` : epoch courante pour la file differee
    pub fn collect_orphans(
        &self,
        scan_snapshot: &EpochScanSnapshot,
        current_epoch: EpochId,
    ) -> ExofsResult<OrphanResult> {
        // Ensemble des ObjectIds atteignables depuis les EpochRoots.
        let reachable: BTreeSet<ObjectId> = scan_snapshot
            .live_objects()
            .map(|ro| ro.object_id)
            .collect();

        // Ensemble des ObjectIds supprimes.
        let deleted: &BTreeSet<ObjectId> = &scan_snapshot.deleted_set;

        // Tous les objets connus dans le reference tracker.
        let all_objects: Vec<ObjectId> = REFERENCE_TRACKER.all_objects();

        let mut result = OrphanResult::default();
        result.objects_analyzed = all_objects.len() as u64;
        result.objects_reachable = reachable.len() as u64;

        // Identifier les orphelins : non atteignable et non dans deleted
        // (les deleted sont traites par le sweeper).
        let mut orphan_blobs: BTreeSet<BlobId> = BTreeSet::new();

        for &oid in &all_objects {
            if reachable.contains(&oid) || deleted.contains(&oid) {
                continue;
            }

            // Cet objet est orphelin.
            result.objects_orphaned = result.objects_orphaned.saturating_add(1);

            // Resoudre ses blobs.
            let blobs = REFERENCE_TRACKER.get_obj_refs(&oid);
            for &bid in &blobs {
                orphan_blobs.insert(bid);
            }

            // Aussi les blobs atteignables via cet objet (sous-blobs).
            if let Ok(reachable_blobs) = REFERENCE_TRACKER.all_reachable_blobs(&oid) {
                for bid in reachable_blobs {
                    orphan_blobs.insert(bid);
                }
            }
        }

        result.blobs_orphaned = orphan_blobs.len() as u64;

        // Traiter les blobs orphelins par batches.
        let orphan_vec: Vec<BlobId> = orphan_blobs.into_iter().collect();
        let mut processed = 0usize;

        for batch_start in (0..orphan_vec.len()).step_by(ORPHAN_BATCH_SIZE) {
            if processed >= MAX_ORPHANS_PER_PASS {
                break;
            }

            let batch_end = (batch_start + ORPHAN_BATCH_SIZE).min(orphan_vec.len());
            let batch = &orphan_vec[batch_start..batch_end];

            for &blob_id in batch {
                if processed >= MAX_ORPHANS_PER_PASS {
                    break;
                }

                let (create_epoch, phys_size) =
                    BLOB_REFCOUNT.get_epoch_and_size(&blob_id);

                // GC-07 : ne pas supprimer si epoch pinnee.
                if is_epoch_pinned(create_epoch) {
                    result.pinned_skipped =
                        result.pinned_skipped.saturating_add(1);
                    processed = processed.saturating_add(1);
                    continue;
                }

                // Decrementer et differer via BLOB_REFCOUNT (GC-01).
                match BLOB_REFCOUNT.dec(&blob_id, current_epoch) {
                    Ok(deferred) => {
                        if deferred.0 == 0 {
                            result.blobs_deferred =
                                result.blobs_deferred.saturating_add(1);
                            result.bytes_deferred =
                                result.bytes_deferred.saturating_add(phys_size);
                        }
                    }
                    Err(ExofsError::Resource) => {
                        result.deferred_full_errs =
                            result.deferred_full_errs.saturating_add(1);
                    }
                    Err(_) => {
                        result.deferred_full_errs =
                            result.deferred_full_errs.saturating_add(1);
                    }
                }

                processed = processed.saturating_add(1);
            }
        }

        result.phase_complete = processed < MAX_ORPHANS_PER_PASS;

        // Mise a jour des metriques.
        GC_METRICS.add_blobs_collected(result.blobs_deferred);
        GC_METRICS.add_bytes_freed(result.bytes_deferred);
        GC_METRICS.add_orphans_collected(result.objects_orphaned);
        GC_METRICS.add_pinned_skipped(result.pinned_skipped);

        // Enregistrement dans l'etat GC.
        GC_STATE.record_orphans(result.objects_orphaned);

        // Stats internes.
        {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            let t = &mut g.total_result;
            t.objects_analyzed = t.objects_analyzed
                .saturating_add(result.objects_analyzed);
            t.objects_reachable = t.objects_reachable
                .saturating_add(result.objects_reachable);
            t.objects_orphaned = t.objects_orphaned
                .saturating_add(result.objects_orphaned);
            t.blobs_orphaned = t.blobs_orphaned
                .saturating_add(result.blobs_orphaned);
            t.blobs_deferred = t.blobs_deferred
                .saturating_add(result.blobs_deferred);
            t.pinned_skipped = t.pinned_skipped
                .saturating_add(result.pinned_skipped);
            t.bytes_deferred = t.bytes_deferred
                .saturating_add(result.bytes_deferred);
            t.deferred_full_errs = t.deferred_full_errs
                .saturating_add(result.deferred_full_errs);
        }

        Ok(result)
    }

    // ── Collecte des blobs orphelins seuls ───────────────────────────────────

    /// Identifie les BlobIds non referencies par aucun objet ni sous-blob connu.
    ///
    /// Utile pour nettoyer les blobs qui ont ete desattaches de tous leurs objets.
    pub fn find_unreferenced_blobs(
        &self,
        current_epoch: EpochId,
    ) -> ExofsResult<u64> {
        let all_blobs: Vec<BlobId> = REFERENCE_TRACKER.all_blobs();
        let mut freed: u64 = 0;

        for blob_id in all_blobs {
            let rc = BLOB_REFCOUNT.get_count(&blob_id);
            if rc != 0 {
                continue;
            }

            let (create_epoch, _phys_size) =
                BLOB_REFCOUNT.get_epoch_and_size(&blob_id);

            if is_epoch_pinned(create_epoch) {
                continue;
            }

            match BLOB_REFCOUNT.dec(&blob_id, current_epoch) {
                Ok(deferred) => {
                    if deferred.0 == 0 {
                        freed = freed.saturating_add(1);
                    }
                }
                Err(_) => {}
            }
        }

        Ok(freed)
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    pub fn total_result(&self) -> OrphanResult {
        self.inner.lock().total_result.clone()
    }

    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    pub fn reset_stats(&self) {
        let mut g = self.inner.lock();
        g.total_result = OrphanResult::default();
        g.pass_count = 0;
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Collecteur d'orphelins GC global.
pub static ORPHAN_COLLECTOR: OrphanCollector = OrphanCollector::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, ObjectId};
    use crate::fs::exofs::gc::epoch_scanner::EpochScanSnapshot;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32]; a[0] = b; ObjectId(a)
    }

    fn bid(b: u8) -> BlobId {
        let mut a = [0u8; 32]; a[0] = b; BlobId(a)
    }

    #[test]
    fn test_empty_scan_no_objects() {
        let collector = OrphanCollector::new();
        let scan = EpochScanSnapshot::empty();
        let result = collector.collect_orphans(&scan, 5).unwrap();
        assert_eq!(result.objects_analyzed, 0);
        assert_eq!(result.objects_orphaned, 0);
    }

    #[test]
    fn test_orphan_display() {
        let r = OrphanResult {
            objects_analyzed:   10,
            objects_reachable:  7,
            objects_orphaned:   3,
            blobs_orphaned:     5,
            blobs_deferred:     4,
            pinned_skipped:     1,
            bytes_deferred:     4096,
            deferred_full_errs: 0,
            phase_complete:     true,
        };
        let s = alloc::format!("{}", r);
        assert!(s.contains("orph_obj=3"));
    }

    #[test]
    fn test_stats_initial() {
        let collector = OrphanCollector::new();
        let t = collector.total_result();
        assert_eq!(t.objects_orphaned, 0);
        assert_eq!(t.blobs_deferred, 0);
    }

    #[test]
    fn test_reset_stats() {
        let collector = OrphanCollector::new();
        // On ne peut pas vraiment incrementer les stats en test unitaire sans
        // REFERENCE_TRACKER global initialisé, donc on verifie juste le reset.
        collector.reset_stats();
        assert_eq!(collector.pass_count(), 0);
    }
}
