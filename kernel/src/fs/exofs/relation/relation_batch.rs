//! relation_batch.rs — Atomic batch operations on ExoFS relations
//!
//! Applied rules:
//!  - OOM-02   : try_reserve before any push
//!  - ARITH-02 : checked arithmetic

extern crate alloc;
use alloc::vec::Vec;

use super::relation::{Relation, RelationId};
use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;
use super::relation_storage::RELATION_STORAGE;
use super::relation_type::{RelationKind, RelationType};
use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01: replaces arch::time
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum batch size.
pub const BATCH_MAX_OPS: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// BatchOp — elementary operation
// ─────────────────────────────────────────────────────────────────────────────

/// Elementary operation of a batch.
#[derive(Clone, Debug)]
pub enum BatchOp {
    /// Insert a relation from -> to of type `rel_type`.
    Insert {
        from: BlobId,
        to: BlobId,
        rel_type: RelationType,
    },
    /// Remove relation identified by `id`.
    Remove { id: RelationId },
    /// Update existing relation (replaces with new version).
    Update { rel: Relation },
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Behavior on partial error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchPolicy {
    /// Continues subsequent operations even on error.
    BestEffort,
    /// Stops at first failure (no partial rollback).
    FailFast,
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchResult
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a batch.
#[derive(Clone, Debug, Default)]
pub struct BatchResult {
    /// Number of successful insertions.
    pub inserted: u32,
    /// Number of successful removals.
    pub removed: u32,
    /// Number of successful updates.
    pub updated: u32,
    /// Number of failed operations.
    pub failed: u32,
    /// First error code encountered (if applicable).
    pub first_err: Option<ExofsError>,
}

impl BatchResult {
    /// `true` if all operations succeeded.
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }

    /// Total number of operations performed (successful + failed).
    pub fn total_ops(&self) -> u32 {
        self.inserted
            .saturating_add(self.removed)
            .saturating_add(self.updated)
            .saturating_add(self.failed)
    }

    /// Number of successful operations.
    pub fn success_count(&self) -> u32 {
        self.inserted
            .saturating_add(self.removed)
            .saturating_add(self.updated)
    }

    fn record_err(&mut self, e: ExofsError) {
        self.failed = self.failed.saturating_add(1);
        if self.first_err.is_none() {
            self.first_err = Some(e);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchStats — cumulative statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Cumulative statistics over all executed batches.
#[derive(Clone, Debug, Default)]
pub struct BatchStats {
    pub total_batches: u64,
    pub total_ops: u64,
    pub total_inserted: u64,
    pub total_removed: u64,
    pub total_updated: u64,
    pub total_failed: u64,
}

impl BatchStats {
    #[allow(dead_code)]
    fn record(&mut self, res: &BatchResult) {
        self.total_batches = self.total_batches.wrapping_add(1);
        self.total_inserted = self.total_inserted.wrapping_add(res.inserted as u64);
        self.total_removed = self.total_removed.wrapping_add(res.removed as u64);
        self.total_updated = self.total_updated.wrapping_add(res.updated as u64);
        self.total_failed = self.total_failed.wrapping_add(res.failed as u64);
        self.total_ops = self.total_ops.wrapping_add(res.total_ops() as u64);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationBatch
// ─────────────────────────────────────────────────────────────────────────────

/// Batch of operations on relations.
///
/// Operations are accumulated then executed in a single pass
/// via `commit()`. In `FailFast` mode, execution stops at the
/// first error without partial rollback.
pub struct RelationBatch {
    ops: Vec<BatchOp>,
    policy: BatchPolicy,
}

impl RelationBatch {
    /// Creates an empty batch with default policy (`BestEffort`).
    pub fn new() -> Self {
        RelationBatch {
            ops: Vec::new(),
            policy: BatchPolicy::BestEffort,
        }
    }

    /// Creates a batch with an explicit policy.
    pub fn with_policy(policy: BatchPolicy) -> Self {
        RelationBatch {
            ops: Vec::new(),
            policy,
        }
    }

    /// Adds an insertion operation.
    pub fn add_insert(
        &mut self,
        from: BlobId,
        to: BlobId,
        rel_type: RelationType,
    ) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Insert { from, to, rel_type });
        Ok(())
    }

    /// Adds a removal operation.
    pub fn add_remove(&mut self, id: RelationId) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Remove { id });
        Ok(())
    }

    /// Adds an update operation.
    pub fn add_update(&mut self, rel: Relation) -> ExofsResult<()> {
        if self.ops.len() >= BATCH_MAX_OPS {
            return Err(ExofsError::NoSpace);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(BatchOp::Update { rel });
        Ok(())
    }

    /// Number of pending operations.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// `true` if batch is empty.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Clears all operations without executing them.
    pub fn clear(&mut self) {
        self.ops.clear();
    }

    /// Executes all operations in insertion order.
    ///
    /// Returns a summary `BatchResult`.
    /// The batch is consumed.
    pub fn commit(self) -> BatchResult {
        let policy = self.policy;
        let mut res = BatchResult::default();
        let tick = exofs_ticks();

        for op in self.ops {
            let outcome = Self::execute_op(op, tick);
            match outcome {
                Ok(op_kind) => match op_kind {
                    OpKind::Insert => res.inserted = res.inserted.saturating_add(1),
                    OpKind::Remove => res.removed = res.removed.saturating_add(1),
                    OpKind::Update => res.updated = res.updated.saturating_add(1),
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

    // ── Internal ─────────────────────────────────────────────────────────────

    fn execute_op(op: BatchOp, tick: u64) -> ExofsResult<OpKind> {
        match op {
            BatchOp::Insert { from, to, rel_type } => {
                let id = RELATION_STORAGE.allocate_id();
                let rel = Relation::new(id, from, to, rel_type, tick);
                RELATION_STORAGE.persist(&rel)?;
                if let Err(e) = RELATION_GRAPH.add_relation(&rel) {
                    RELATION_STORAGE.remove(id);
                    return Err(e);
                }
                if let Err(e) = RELATION_INDEX.insert(&rel) {
                    RELATION_GRAPH.remove_relation(&rel);
                    RELATION_STORAGE.remove(id);
                    return Err(e);
                }
                Ok(OpKind::Insert)
            }
            BatchOp::Remove { id } => {
                let rel = RELATION_STORAGE
                    .load(id)
                    .ok_or(ExofsError::ObjectNotFound)??;
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(id);
                Ok(OpKind::Remove)
            }
            BatchOp::Update { rel } => {
                if let Some(old_res) = RELATION_STORAGE.load(rel.id) {
                    if let Ok(old_rel) = old_res {
                        RELATION_GRAPH.remove_relation(&old_rel);
                        RELATION_INDEX.remove(&old_rel);
                    }
                }
                RELATION_STORAGE.update(&rel)?;
                if rel.is_active() {
                    let _ = RELATION_GRAPH.add_relation(&rel);
                    let _ = RELATION_INDEX.insert(&rel);
                }
                Ok(OpKind::Update)
            }
        }
    }
}

/// Type of successful operation (internal use).
enum OpKind {
    Insert,
    Remove,
    Update,
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchBuilder — fluent API
// ─────────────────────────────────────────────────────────────────────────────

/// Fluent builder to assemble a batch quickly.
pub struct BatchBuilder {
    batch: RelationBatch,
    error: Option<ExofsError>,
}

impl BatchBuilder {
    /// Creates a builder.
    pub fn new() -> Self {
        BatchBuilder {
            batch: RelationBatch::new(),
            error: None,
        }
    }

    /// Sets error policy.
    pub fn policy(mut self, p: BatchPolicy) -> Self {
        self.batch.policy = p;
        self
    }

    /// Adds an insertion.
    pub fn insert(mut self, from: BlobId, to: BlobId, kind: RelationKind) -> Self {
        if self.error.is_none() {
            let rt = RelationType::new(kind);
            if let Err(e) = self.batch.add_insert(from, to, rt) {
                self.error = Some(e);
            }
        }
        self
    }

    /// Adds a removal.
    pub fn remove(mut self, id: RelationId) -> Self {
        if self.error.is_none() {
            if let Err(e) = self.batch.add_remove(id) {
                self.error = Some(e);
            }
        }
        self
    }

    /// Verifies and returns the batch ready to be executed.
    pub fn build(self) -> ExofsResult<RelationBatch> {
        if let Some(e) = self.error {
            return Err(e);
        }
        Ok(self.batch)
    }

    /// Builds and executes directly.
    pub fn execute(self) -> ExofsResult<BatchResult> {
        Ok(self.build()?.commit())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience functions
// ─────────────────────────────────────────────────────────────────────────────

/// Creates and immediately executes a batch containing a single Insert.
pub fn insert_relation_now(from: BlobId, to: BlobId, rel_type: RelationType) -> ExofsResult<()> {
    let mut b = RelationBatch::new();
    b.add_insert(from, to, rel_type)?;
    let res = b.commit();
    if res.failed > 0 {
        Err(res.first_err.unwrap_or(ExofsError::InternalError))
    } else {
        Ok(())
    }
}

/// Removes a list of relations in a single batch.
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
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::relation_type::RelationType;
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    #[test]
    fn test_batch_empty() {
        let b = RelationBatch::new();
        assert!(b.is_empty());
        let res = b.commit();
        assert!(res.is_success());
        assert_eq!(res.total_ops(), 0);
    }

    #[test]
    fn test_batch_add_ops() {
        let mut b = RelationBatch::new();
        b.add_insert(blob(1), blob(2), RelationType::new(RelationKind::Parent))
            .test_unwrap();
        b.add_insert(blob(3), blob(4), RelationType::new(RelationKind::Clone))
            .test_unwrap();
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn test_batch_remove_not_found_best_effort() {
        let mut b = RelationBatch::with_policy(BatchPolicy::BestEffort);
        b.add_remove(RelationId(9999999)).test_unwrap();
        let res = b.commit();
        // BestEffort: no panic, just a failed count.
        assert_eq!(res.failed, 1);
    }

    #[test]
    fn test_batch_remove_not_found_fail_fast() {
        let mut b = RelationBatch::with_policy(BatchPolicy::FailFast);
        b.add_remove(RelationId(8888888)).test_unwrap();
        b.add_remove(RelationId(7777777)).test_unwrap();
        let res = b.commit();
        // FailFast: stops at first failure.
        assert!(res.first_err.is_some());
        assert!(res.failed >= 1);
    }

    #[test]
    fn test_batch_result_stats() {
        let res = BatchResult {
            inserted: 3,
            removed: 2,
            updated: 1,
            failed: 1,
            first_err: Some(ExofsError::NoSpace),
        };
        assert_eq!(res.total_ops(), 7);
        assert_eq!(res.success_count(), 6);
        assert!(!res.is_success());
    }

    #[test]
    fn test_builder_basic() {
        let b = BatchBuilder::new()
            .policy(BatchPolicy::BestEffort)
            .remove(RelationId(111))
            .build()
            .test_unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn test_builder_insert() {
        let b = BatchBuilder::new()
            .insert(blob(10), blob(11), RelationKind::Snapshot)
            .build()
            .test_unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn test_remove_batch_empty() {
        let res = remove_relations_batch(&[]);
        assert!(res.is_success());
    }

    #[test]
    fn test_batch_max_ops() {
        let mut b = RelationBatch::new();
        for _i in 0..BATCH_MAX_OPS {
            b.add_insert(blob(0), blob(1), RelationType::new(RelationKind::Parent))
                .test_unwrap();
        }
        // Next one must fail.
        let err = b.add_insert(blob(0), blob(1), RelationType::new(RelationKind::Parent));
        assert!(err.is_err());
    }

    #[test]
    fn test_batch_clear() {
        let mut b = RelationBatch::new();
        b.add_remove(RelationId(42)).test_unwrap();
        b.clear();
        assert!(b.is_empty());
    }
}
