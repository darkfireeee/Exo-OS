//! RelationBatch — insertion/suppression atomique de relations ExoFS (no_std).

use alloc::vec::Vec;
use crate::arch::time::read_ticks;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation::{Relation, RelationId};
use super::relation_type::RelationType;
use super::relation_storage::RELATION_STORAGE;
use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;

/// Résultat d'un batch.
#[derive(Clone, Debug)]
pub struct BatchResult {
    pub inserted: u32,
    pub removed:  u32,
    pub failed:   u32,
}

/// Opération dans un batch.
pub enum BatchOp {
    Insert { from: BlobId, to: BlobId, rel_type: RelationType },
    Remove { id: RelationId },
}

pub struct RelationBatch {
    ops: Vec<BatchOp>,
}

impl RelationBatch {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn add_insert(
        &mut self,
        from: BlobId,
        to: BlobId,
        rel_type: RelationType,
    ) -> Result<(), FsError> {
        self.ops.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        self.ops.push(BatchOp::Insert { from, to, rel_type });
        Ok(())
    }

    pub fn add_remove(&mut self, id: RelationId) -> Result<(), FsError> {
        self.ops.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        self.ops.push(BatchOp::Remove { id });
        Ok(())
    }

    /// Exécute le batch ; ne fait pas de rollback partiel (best-effort).
    pub fn commit(self) -> BatchResult {
        let mut res = BatchResult { inserted: 0, removed: 0, failed: 0 };
        for op in self.ops {
            match op {
                BatchOp::Insert { from, to, rel_type } => {
                    let id = RELATION_STORAGE.allocate_id();
                    let rel = Relation::new(id, from, to, rel_type, read_ticks());
                    let ok = RELATION_STORAGE.persist(&rel).is_ok()
                          && RELATION_GRAPH.add_relation(&rel).is_ok()
                          && RELATION_INDEX.insert(&rel).is_ok();
                    if ok { res.inserted += 1; } else { res.failed += 1; }
                }
                BatchOp::Remove { id } => {
                    if let Some(rel) = RELATION_STORAGE.load(id) {
                        RELATION_GRAPH.remove_relation(&rel);
                        RELATION_INDEX.remove(&rel);
                        RELATION_STORAGE.remove(id);
                        res.removed += 1;
                    } else {
                        res.failed += 1;
                    }
                }
            }
        }
        res
    }
}
