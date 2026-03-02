//! RelationQuery — requêtes haut niveau sur le graphe de relations ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation::{Relation, RelationId};
use super::relation_type::RelationKind;
use super::relation_storage::RELATION_STORAGE;
use super::relation_index::RELATION_INDEX;

/// Résultat d'une requête de relations.
#[derive(Clone, Debug)]
pub struct QueryResult {
    pub relations: Vec<Relation>,
    pub n_total:   usize,
}

pub struct RelationQuery;

impl RelationQuery {
    /// Toutes les relations sortantes d'un blob.
    pub fn outgoing(from: &BlobId) -> Result<QueryResult, FsError> {
        let ids = RELATION_INDEX.ids_from(from);
        Self::load_ids(&ids)
    }

    /// Toutes les relations entrantes vers un blob.
    pub fn incoming(to: &BlobId) -> Result<QueryResult, FsError> {
        let ids = RELATION_INDEX.ids_to(to);
        Self::load_ids(&ids)
    }

    /// Toutes les relations sortantes d'un blob d'un type donné.
    pub fn outgoing_by_kind(from: &BlobId, kind: RelationKind) -> Result<QueryResult, FsError> {
        let ids = RELATION_INDEX.ids_from(from);
        let mut rels = Vec::new();
        for id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                if r.rel_type.kind == kind {
                    rels.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    rels.push(r);
                }
            }
        }
        let n_total = rels.len();
        Ok(QueryResult { relations: rels, n_total })
    }

    /// Trouve tous les blobs accessibles depuis `start` (fermeture transitive).
    pub fn find_all_reachable(start: &BlobId, max_depth: u32) -> Result<Vec<BlobId>, FsError> {
        use super::relation_walker::RelationWalker;
        let walker = RelationWalker::new(max_depth);
        let result = walker.bfs(*start)?;
        Ok(result.visited)
    }

    /// Vérifie s'il existe une relation directe from→to.
    pub fn has_direct_relation(from: &BlobId, to: &BlobId, kind: RelationKind) -> bool {
        let ids = RELATION_INDEX.ids_from(from);
        for id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                if r.to.as_bytes() == to.as_bytes() && r.rel_type.kind == kind {
                    return true;
                }
            }
        }
        false
    }

    fn load_ids(ids: &[RelationId]) -> Result<QueryResult, FsError> {
        let mut rels = Vec::new();
        rels.try_reserve(ids.len()).map_err(|_| FsError::OutOfMemory)?;
        for &id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                rels.push(r);
            }
        }
        let n_total = rels.len();
        Ok(QueryResult { relations: rels, n_total })
    }
}
