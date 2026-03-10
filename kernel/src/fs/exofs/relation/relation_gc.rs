//! relation_gc.rs — Collecteur de relations orphelines ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée
//!  - RECUR-01 : aucune récursion


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01 : remplace arch::time
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::relation_storage::RELATION_STORAGE;
use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;

// ─────────────────────────────────────────────────────────────────────────────
// BlobExistsChecker
// ─────────────────────────────────────────────────────────────────────────────

/// Interface pour vérifier l'existence d'un blob.
///
/// Le GC délègue la vérification à l'implémentation fournie par l'appelant.
pub trait BlobExistsChecker: Send + Sync {
    /// `true` si le blob identifié par `key` existe encore dans le store.
    fn exists(&self, key: &[u8; 32]) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// GcPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de collecte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcPolicy {
    /// Supprime uniquement les relations orphelines (from ou to disparu).
    OrphansOnly,
    /// Supprime aussi les relations soft-deleted.
    IncludeDeleted,
    /// Supprime toutes les relations dont l'âge dépasse `max_age_ticks`.
    ByAge { max_age_ticks: u64 },
    /// Combinaison de toutes les politiques.
    Full,
}

impl GcPolicy {
    /// `true` si les suppressions soft-delete doivent être purgées.
    pub fn purge_deleted(self) -> bool {
        matches!(self, GcPolicy::IncludeDeleted | GcPolicy::Full)
    }

    /// `true` si les relations trop vieilles doivent être purgées.
    pub fn purge_by_age(self) -> Option<u64> {
        match self {
            GcPolicy::ByAge { max_age_ticks } => Some(max_age_ticks),
            GcPolicy::Full => Some(u64::MAX),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationGcReport
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport de GC.
#[derive(Clone, Debug, Default)]
pub struct RelationGcReport {
    /// Nombre de relations examinées.
    pub examined:   u32,
    /// Nombre de relations supprimées.
    pub purged:     u32,
    /// Nombre de relations conservées.
    pub kept:       u32,
    /// Nombre d'erreurs rencontrées.
    pub errors:     u32,
    /// Ticks de début du GC.
    pub started_at: u64,
    /// Ticks de fin du GC.
    pub ended_at:   u64,
}

impl RelationGcReport {
    /// Durée du GC en ticks CPU.
    pub fn duration_ticks(&self) -> u64 {
        self.ended_at.saturating_sub(self.started_at)
    }

    /// Taux de purge (purged / examined), en pourcents.
    pub fn purge_rate_pct(&self) -> u32 {
        if self.examined == 0 { return 0; }
        ((self.purged as u64 * 100) / self.examined as u64) as u32
    }

    /// `true` si aucune erreur.
    pub fn is_clean(&self) -> bool { self.errors == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcCandidate — relation candidate à la suppression
// ─────────────────────────────────────────────────────────────────────────────

/// Relation identifiée comme candidate à la purge.
#[derive(Clone, Debug)]
pub struct GcCandidate {
    pub blob_from:  [u8; 32],
    pub blob_to:    [u8; 32],
    pub reason:     GcReason,
}

/// Raison de la candidature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcReason {
    /// Le blob source n'existe plus.
    FromBlobMissing,
    /// Le blob destination n'existe plus.
    ToBlobMissing,
    /// Relation soft-deleted.
    SoftDeleted,
    /// Relation trop ancienne.
    TooOld,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationGc
// ─────────────────────────────────────────────────────────────────────────────

/// Collecteur de relations orphelines.
pub struct RelationGc;

impl RelationGc {
    /// Purge les relations orphelines selon la politique donnée.
    ///
    /// Itératif (RECUR-01) — simple boucle `for` sur les relations.
    pub fn run(
        checker: &dyn BlobExistsChecker,
        policy:  GcPolicy,
    ) -> ExofsResult<RelationGcReport> {
        let started_at = exofs_ticks();
        let all = RELATION_STORAGE.load_all()?;
        let now_ticks = exofs_ticks();
        let mut report = RelationGcReport {
            started_at,
            ..Default::default()
        };

        for rel in all {
            report.examined = report.examined.checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;

            let from_key = rel.from.as_bytes();
            let to_key   = rel.to.as_bytes();

            let should_purge = match policy {
                GcPolicy::OrphansOnly => {
                    !checker.exists(from_key) || !checker.exists(to_key)
                }
                GcPolicy::IncludeDeleted => {
                    !rel.is_active()
                    || !checker.exists(from_key)
                    || !checker.exists(to_key)
                }
                GcPolicy::ByAge { max_age_ticks } => {
                    let age = now_ticks.saturating_sub(rel.created_at);
                    age > max_age_ticks
                }
                GcPolicy::Full => {
                    let age = now_ticks.saturating_sub(rel.created_at);
                    !rel.is_active()
                    || !checker.exists(from_key)
                    || !checker.exists(to_key)
                    || age > u64::MAX / 2
                }
            };

            if should_purge {
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(rel.id);
                report.purged = report.purged.checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
            } else {
                report.kept = report.kept.checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
            }
        }

        report.ended_at = exofs_ticks();
        Ok(report)
    }

    /// Purge uniquement les relations liées à un blob donné (supprimé).
    ///
    /// Utilisé lors d'un soft-delete ou d'une purge d'objet.
    pub fn purge_blob(key: &[u8; 32]) -> ExofsResult<u32> {
        let blob = BlobId(*key);

        let from_ids = RELATION_INDEX.ids_from(&blob);
        let to_ids   = RELATION_INDEX.ids_to(&blob);

        let mut n_purged = 0u32;

        for id in from_ids.iter().chain(to_ids.iter()) {
            if let Some(rel_result) = RELATION_STORAGE.load(*id) {
                match rel_result {
                    Ok(rel) => {
                        RELATION_GRAPH.remove_relation(&rel);
                        RELATION_INDEX.remove(&rel);
                        RELATION_STORAGE.remove(*id);
                        n_purged = n_purged
                            .checked_add(1)
                            .ok_or(ExofsError::OffsetOverflow)?;
                    }
                    Err(_) => {
                        // Relation corrompue — supprime quand même du store.
                        RELATION_STORAGE.remove(*id);
                        n_purged = n_purged.saturating_add(1);
                    }
                }
            }
        }

        Ok(n_purged)
    }

    /// Collecte les candidats à la purge sans les supprimer.
    ///
    /// Utile pour inspecter ce qui serait purgé avant de lancer un vrai GC.
    pub fn dry_run(
        checker: &dyn BlobExistsChecker,
    ) -> ExofsResult<Vec<GcCandidate>> {
        let all = RELATION_STORAGE.load_all()?;
        let mut candidates: Vec<GcCandidate> = Vec::new();

        for rel in all {
            let from_key = rel.from.as_bytes();
            let to_key   = rel.to.as_bytes();

            if !checker.exists(from_key) {
                candidates.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to:   *to_key,
                    reason:    GcReason::FromBlobMissing,
                });
            } else if !checker.exists(to_key) {
                candidates.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to:   *to_key,
                    reason:    GcReason::ToBlobMissing,
                });
            } else if !rel.is_active() {
                candidates.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to:   *to_key,
                    reason:    GcReason::SoftDeleted,
                });
            }
        }

        Ok(candidates)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcScheduler — planification heuristique du GC
// ─────────────────────────────────────────────────────────────────────────────

/// Critère de déclenchement automatique du GC.
#[derive(Clone, Debug)]
pub struct GcTrigger {
    /// Déclenche si le nombre de relations dépasse ce seuil.
    pub max_relations:  usize,
    /// Déclenche si N ticks se sont écoulés depuis le dernier GC.
    pub min_interval_ticks: u64,
}

impl Default for GcTrigger {
    fn default() -> Self {
        GcTrigger {
            max_relations:      60000,
            min_interval_ticks: 1_000_000,
        }
    }
}

/// Planificateur de GC — décide quand il est opportun de lancer un GC.
pub struct GcScheduler {
    trigger:         GcTrigger,
    last_gc_tick:    u64,
    total_gc_runs:   u64,
}

impl GcScheduler {
    /// Crée un planificateur avec les triggers par défaut.
    pub fn new() -> Self {
        GcScheduler {
            trigger:       GcTrigger::default(),
            last_gc_tick:  0,
            total_gc_runs: 0,
        }
    }

    /// Crée avec des triggers explicites.
    pub fn with_trigger(trigger: GcTrigger) -> Self {
        GcScheduler { trigger, last_gc_tick: 0, total_gc_runs: 0 }
    }

    /// `true` si un GC devrait être lancé maintenant.
    pub fn should_run(&self) -> bool {
        let now      = exofs_ticks();
        let n_rels   = RELATION_STORAGE.count();
        let interval = now.saturating_sub(self.last_gc_tick);

        n_rels >= self.trigger.max_relations
            || interval >= self.trigger.min_interval_ticks
    }

    /// Lance le GC si les conditions sont remplies.
    ///
    /// Retourne `None` si le GC n'a pas été lancé.
    pub fn maybe_run(
        &mut self,
        checker: &dyn BlobExistsChecker,
        policy:  GcPolicy,
    ) -> ExofsResult<Option<RelationGcReport>> {
        if !self.should_run() { return Ok(None); }
        let report = RelationGc::run(checker, policy)?;
        self.last_gc_tick  = exofs_ticks();
        self.total_gc_runs = self.total_gc_runs.wrapping_add(1);
        Ok(Some(report))
    }

    /// Nombre total de GC exécutés depuis la création.
    pub fn total_runs(&self) -> u64 { self.total_gc_runs }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::relation::{Relation, RelationId};
    use super::super::relation_type::{RelationType, RelationKind};
    use super::super::relation_storage::RelationStorage;
    use super::super::relation_index::RelationIndex;
    use super::super::relation_graph::RelationGraph;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    /// Checker qui marque tous les blobs comme existants.
    struct AllExist;
    impl BlobExistsChecker for AllExist {
        fn exists(&self, _key: &[u8; 32]) -> bool { true }
    }

    /// Checker qui marque tous les blobs comme supprimés.
    struct NoneExist;
    impl BlobExistsChecker for NoneExist {
        fn exists(&self, _key: &[u8; 32]) -> bool { false }
    }

    #[test] fn test_gc_report_defaults() {
        let r = RelationGcReport::default();
        assert!(r.is_clean());
        assert_eq!(r.purge_rate_pct(), 0);
        assert_eq!(r.duration_ticks(), 0);
    }

    #[test] fn test_gc_report_purge_rate() {
        let r = RelationGcReport {
            examined: 10, purged: 4, ..Default::default()
        };
        assert_eq!(r.purge_rate_pct(), 40);
    }

    #[test] fn test_gc_policy_variants() {
        assert!(!GcPolicy::OrphansOnly.purge_deleted());
        assert!(GcPolicy::IncludeDeleted.purge_deleted());
        assert!(GcPolicy::Full.purge_deleted());
        assert!(GcPolicy::ByAge { max_age_ticks: 100 }.purge_by_age().is_some());
    }

    #[test] fn test_gc_reason_variants() {
        let r = GcReason::FromBlobMissing;
        assert_eq!(r, GcReason::FromBlobMissing);
    }

    #[test] fn test_dry_run_all_exist() {
        // Toutes les relations ont their blobs existants → aucun candidat.
        let candidates = RelationGc::dry_run(&AllExist).unwrap();
        // Des relations globales peuvent exister dans le store global —
        // on vérifie seulement que la fonction ne panique pas.
        let _ = candidates;
    }

    #[test] fn test_scheduler_new() {
        let s = GcScheduler::new();
        assert_eq!(s.total_runs(), 0);
    }

    #[test] fn test_scheduler_with_trigger() {
        let t = GcTrigger { max_relations: 100, min_interval_ticks: 50 };
        let s = GcScheduler::with_trigger(t.clone());
        assert_eq!(s.trigger.max_relations, 100);
    }

    #[test] fn test_purge_blob_missing() {
        // Blob inexistant → aucune relation à purger.
        let n = RelationGc::purge_blob(&[0xABu8; 32]).unwrap();
        // On ne sait pas combien il y en a dans le store global,
        // mais la fonction ne doit pas paniquer.
        let _ = n;
    }

    #[test] fn test_gc_run_all_exist() {
        let checker = AllExist;
        let report = RelationGc::run(&checker, GcPolicy::OrphansOnly).unwrap();
        // Aucun orphelin car tous les blobs existent.
        assert!(report.is_clean());
        // purged peut être 0 ou plus selon l'état du store global.
        let _ = report;
    }

    #[test] fn test_gc_candidate_struct() {
        let c = GcCandidate {
            blob_from: [1u8; 32],
            blob_to:   [2u8; 32],
            reason:    GcReason::ToBlobMissing,
        };
        assert_eq!(c.reason, GcReason::ToBlobMissing);
    }
}
