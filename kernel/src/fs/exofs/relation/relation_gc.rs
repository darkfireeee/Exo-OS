//! RelationGc — nettoyage des relations orphelines ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::relation_storage::RELATION_STORAGE;
use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;

/// Rapport de GC des relations.
#[derive(Clone, Debug, Default)]
pub struct RelationGcReport {
    pub examined:  u32,
    pub purged:    u32,
    pub kept:      u32,
    pub errors:    u32,
}

/// Interface pour vérifier l'existence d'un blob.
pub trait BlobExistsChecker: Send + Sync {
    fn exists(&self, key: &[u8; 32]) -> bool;
}

pub struct RelationGc;

impl RelationGc {
    /// Purge les relations dont `from` ou `to` n'existe plus dans le store.
    pub fn run(checker: &dyn BlobExistsChecker) -> Result<RelationGcReport, FsError> {
        let all = RELATION_STORAGE.load_all()?;
        let mut report = RelationGcReport::default();

        for rel in all {
            report.examined += 1;
            let from_key = rel.from.as_bytes();
            let to_key   = rel.to.as_bytes();

            let orphaned = !checker.exists(&from_key) || !checker.exists(&to_key);
            if orphaned {
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(rel.id);
                report.purged += 1;
            } else {
                report.kept += 1;
            }
        }

        Ok(report)
    }

    /// Purge uniquement les relations associées à un blob connu supprimé.
    pub fn purge_blob(
        key: &[u8; 32],
        checker: &dyn BlobExistsChecker,
    ) -> Result<u32, FsError> {
        use crate::fs::exofs::core::BlobId;
        let blob = BlobId::from_raw(*key);

        let from_ids = RELATION_INDEX.ids_from(&blob);
        let to_ids   = RELATION_INDEX.ids_to(&blob);

        let mut n_purged = 0u32;
        for id in from_ids.iter().chain(to_ids.iter()) {
            if let Some(rel) = RELATION_STORAGE.load(*id) {
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(*id);
                n_purged += 1;
            }
        }
        Ok(n_purged)
    }
}
