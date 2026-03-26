//! relation_batch.rs — Opérations batch atomiques sur les relations ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve avant tout push
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01 : remplace arch::time
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::relation::{Relation, RelationId};
use super::relation_type::{RelationType, RelationKind};
use super::relation_storage::RELATION_STORAGE;
use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un batch.
pub const BATCH_MAX_OPS: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// BatchOp — opération élémentaire
// ─────────────────────────────────────────────────────────────────────────────

/// Opération élémentaire d'un batch.
#[derive(Clone, Debug)]
pub enum BatchOp {
    /// Insérer une relation from → to de type `rel_type`.
    Insert {
        from:     BlobId,
        to:       BlobId,
        rel_type: RelationType,
    },
    /// Supprimer la relation identifiée par `id`.
    Remove {
        id: RelationId,
    },
    /// Mettre à jour une relation existante (remplace par la nouvelle version).
    Update {
        rel: Relation,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Comportement en cas d'erreur partielle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchPolicy {
    /// Continue les opérations suivantes même en cas d'erreur.
    BestEffort,
    /// Arrête au premier échec (pas de rollback partiel).
    FailFast,
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un batch.
#[derive(Clone, Debug, Default)]
pub struct BatchResult {
    /// Nombre d'insertions réussies.
    pub inserted:  u32,
    /// Nombre de suppressions réussies.
    pub removed:   u32,
    /// Nombre de mises à jour réussies.
    pub updated:   u32,
    /// Nombre d'opérations échouées.
    pub failed:    u32,
    /// Premier code d'erreur rencontré (si applicable).
    pub first_err: Option<ExofsError>,
}

impl BatchResult {
    /// `true` si toutes les opérations ont réussi.
    pub fn is_success(&self) -> bool { self.failed == 0 }

    /// Nombre total d'opérations effectuées (réussies + échouées).
    pub fn total_ops(&self) -> u32 {
        self.inserted
            .saturating_add(self.removed)
            .saturating_add(self.updated)
            .saturating_add(self.failed)
    }

    /// Nombre d'opérations réussies.
    pub fn success_count(&self) -> u32 {
        self.inserted
            .saturating_add(self.removed)
            .saturating_add(self.updated)
    }

    fn record_err(&mut self, e: ExofsError) {
        self.failed = self.failed.saturating_add(1);
        if self.first_err.is_none() { self.first_err = Some(e); }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchStats — statistiques cumulées
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques cumulées sur tous les batchs exécutés.
#[derive(Clone, Debug, Default)]
pub struct BatchStats {
    pub total_batches:   u64,
    pub total_ops:       u64,
    pub total_inserted:  u64,
    pub total_removed:   u64,
    pub total_updated:   u64,
    pub total_failed:    u64,
}

impl BatchStats {
    #[allow(dead_code)]
    fn record(&mut self, res: &BatchResult) {
        self.total_batches  = self.total_batches.wrapping_add(1);
        self.total_inserted = self.total_inserted.wrapping_add(res.inserted as u64);
        self.total_removed  = self.total_removed.wrapping_add(res.removed  as u64);
        self.total_updated  = self.total_updated.wrapping_add(res.updated  as u64);
        self.total_failed   = self.total_failed.wrapping_add(res.failed    as u64);
        self.total_ops      = self.total_ops.wrapping_add(res.total_ops() as u64);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationBatch
// ─────────────────────────────────────────────────────────────────────────────

/// Batch d'opérations sur les relations.
///
/// Les opérations sont accumulées puis exécutées en une seule passe
/// via `commit()`.  En mode `FailFast`, l'exécution s'arrête à la
/// première erreur sans rollback partiel.
pub struct RelationBatch {
    ops:    Vec<BatchOp>,
    policy: BatchPolicy,
}

impl RelationBatch {
    /// Crée un batch vide avec la politique par défaut (`BestEffort`).
    pub fn new() -> Self {
        RelationBatch {
            ops:    Vec::new(),
            policy: BatchPolicy::BestEffort,
        }
    }

    /// Crée un batch avec une politique explicite.
    pub fn with_policy(policy: BatchPolicy) -> Self {
        RelationBatch { ops: Vec::new(), policy }
    }

    /// Ajoute une opération d'insertion.
    pub fn add_insert(
        &mut self,
        from:     BlobId,
        to:       BlobId,
        rel_type: RelationType,
    ) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Insert { from, to, rel_type });
        Ok(())
    }

    /// Ajoute une opération de suppression.
    pub fn add_remove(&mut self, id: RelationId) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Remove { id });
        Ok(())
    }

    /// Ajoute une opération de mise à jour.
    pub fn add_update(&mut self, rel: Relation) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Update { rel });
        Ok(())
    }

    /// Nombre d'opérations en attente.
    pub fn len(&self) -> usize { self.ops.len() }

    /// `true` si le batch est vide.
    pub fn is_empty(&self) -> bool { self.ops.is_empty() }

    /// Vide toutes les opérations sans les exécuter.
    pub fn clear(&mut self) { self.ops.clear(); }

    /// Exécute toutes les opérations dans l'ordre d'insertion.
    ///
    /// Retourne un `BatchResult` récapitulatif.
    /// Le batch est consommé.
    pub fn commit(self) -> BatchResult {
        let policy = self.policy;
        let mut res = BatchResult::default();
        let tick    = exofs_ticks();

        for op in self.ops {
            let outcome = Self::execute_op(op, tick);
            match outcome {
                Ok(op_kind) => match op_kind {
                    OpKind::Insert => res.inserted = res.inserted.saturating_add(1),
                    OpKind::Remove => res.removed  = res.removed.saturating_add(1),
                    OpKind::Update => res.updated  = res.updated.saturating_add(1),
                },
                Err(e) => {
                    res.record_err(e);
                    if policy == BatchPolicy::FailFast {
                        break;
                    }
                }
            }
        }
        res
    }

    // ── Interne ──────────────────────────────────────────────────────────────

    fn execute_op(op: BatchOp, tick: u64) -> ExofsResult<OpKind> {
        match op {
            BatchOp::Insert { from, to, rel_type } => {
                let id  = RELATION_STORAGE.allocate_id();
                let rel = Relation::new(id, from, to, rel_type, tick);
                RELATION_STORAGE.persist(&rel)?;
                RELATION_GRAPH.add_relation(&rel)?;
                RELATION_INDEX.insert(&rel)?;
                Ok(OpKind::Insert)
            }
            BatchOp::Remove { id } => {
                let rel = RELATION_STORAGE.load(id)
                    .ok_or(ExofsError::ObjectNotFound)??;
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(id);
                Ok(OpKind::Remove)
            }
            BatchOp::Update { rel } => {
                RELATION_STORAGE.update(&rel)?;
                Ok(OpKind::Update)
            }
        }
    }
}

/// Type d'opération réussie (usage interne).
enum OpKind { Insert, Remove, Update }

// ─────────────────────────────────────────────────────────────────────────────
// BatchBuilder — API fluente
// ─────────────────────────────────────────────────────────────────────────────

/// Constructeur fluent pour assembler rapidement un batch.
pub struct BatchBuilder {
    batch: RelationBatch,
    error: Option<ExofsError>,
}

impl BatchBuilder {
    /// Crée un builder.
    pub fn new() -> Self {
        BatchBuilder {
            batch: RelationBatch::new(),
            error: None,
        }
    }

    /// Définit la politique en cas d'erreur.
    pub fn policy(mut self, p: BatchPolicy) -> Self {
        self.batch.policy = p; self
    }

    /// Ajoute une insertion.
    pub fn insert(mut self, from: BlobId, to: BlobId, kind: RelationKind) -> Self {
        if self.error.is_none() {
            let rt = RelationType::new(kind);
            if let Err(e) = self.batch.add_insert(from, to, rt) {
                self.error = Some(e);
            }
        }
        self
    }

    /// Ajoute une suppression.
    pub fn remove(mut self, id: RelationId) -> Self {
        if self.error.is_none() {
            if let Err(e) = self.batch.add_remove(id) {
                self.error = Some(e);
            }
        }
        self
    }

    /// Vérifie et retourne le batch prêt à être exécuté.
    pub fn build(self) -> ExofsResult<RelationBatch> {
        if let Some(e) = self.error { return Err(e); }
        Ok(self.batch)
    }

    /// Construit et exécute directement.
    pub fn execute(self) -> ExofsResult<BatchResult> {
        Ok(self.build()?.commit())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité
// ─────────────────────────────────────────────────────────────────────────────

/// Crée et exécute immédiatement un batch d'un seul Insert.
pub fn insert_relation_now(
    from:     BlobId,
    to:       BlobId,
    rel_type: RelationType,
) -> ExofsResult<()> {
    let mut b = RelationBatch::new();
    b.add_insert(from, to, rel_type)?;
    let res = b.commit();
    if res.failed > 0 {
        Err(res.first_err.unwrap_or(ExofsError::InternalError))
    } else {
        Ok(())
    }
}

/// Supprime une liste de relations en un seul batch.
pub fn remove_relations_batch(ids: &[RelationId]) -> BatchResult {
    let mut b = RelationBatch::new();
    for &id in ids {
        let _ = b.add_remove(id);
    }
    b.commit()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::relation_type::RelationType;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    #[test] fn test_batch_empty() {
        let b = RelationBatch::new();
        assert!(b.is_empty());
        let res = b.commit();
        assert!(res.is_success());
        assert_eq!(res.total_ops(), 0);
    }

    #[test] fn test_batch_add_ops() {
        let mut b = RelationBatch::new();
        b.add_insert(blob(1), blob(2), RelationType::new(RelationKind::Parent)).unwrap();
        b.add_insert(blob(3), blob(4), RelationType::new(RelationKind::Clone)).unwrap();
        assert_eq!(b.len(), 2);
    }

    #[test] fn test_batch_remove_not_found_best_effort() {
        let mut b = RelationBatch::with_policy(BatchPolicy::BestEffort);
        b.add_remove(RelationId(9999999)).unwrap();
        let res = b.commit();
        // BestEffort : pas de panique, juste un failed.
        assert_eq!(res.failed, 1);
    }

    #[test] fn test_batch_remove_not_found_fail_fast() {
        let mut b = RelationBatch::with_policy(BatchPolicy::FailFast);
        b.add_remove(RelationId(8888888)).unwrap();
        b.add_remove(RelationId(7777777)).unwrap();
        let res = b.commit();
        // FailFast : s'arrête au premier échec.
        assert!(res.first_err.is_some());
        assert!(res.failed >= 1);
    }

    #[test] fn test_batch_result_stats() {
        let res = BatchResult {
            inserted: 3, removed: 2, updated: 1, failed: 1,
            first_err: Some(ExofsError::NoSpace),
        };
        assert_eq!(res.total_ops(), 7);
        assert_eq!(res.success_count(), 6);
        assert!(!res.is_success());
    }

    #[test] fn test_builder_basic() {
        let b = BatchBuilder::new()
            .policy(BatchPolicy::BestEffort)
            .remove(RelationId(111))
            .build()
            .unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test] fn test_builder_insert() {
        let b = BatchBuilder::new()
            .insert(blob(10), blob(11), RelationKind::Snapshot)
            .build()
            .unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test] fn test_remove_batch_empty() {
        let res = remove_relations_batch(&[]);
        assert!(res.is_success());
    }

    #[test] fn test_batch_max_ops() {
        let mut b = RelationBatch::new();
        for _i in 0..BATCH_MAX_OPS {
            b.add_insert(blob(0), blob(1), RelationType::new(RelationKind::Parent)).unwrap();
        }
        // La prochaine doit échouer.
        let err = b.add_insert(blob(0), blob(1), RelationType::new(RelationKind::Parent));
        assert!(err.is_err());
    }

    #[test] fn test_batch_clear() {
        let mut b = RelationBatch::new();
        b.add_remove(RelationId(42)).unwrap();
        b.clear();
        assert!(b.is_empty());
    }
}
