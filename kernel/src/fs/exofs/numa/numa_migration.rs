//! numa_migration.rs — Migration de blobs entre nœuds NUMA (no_std).

use crate::fs::exofs::core::{BlobId, FsError};
use super::numa_stats::NUMA_STATS;

#[derive(Debug)]
pub enum MigrationResult {
    Migrated { from: usize, to: usize, bytes: u64 },
    AlreadyOnTarget,
    NotFound,
    Error(FsError),
}

pub struct NumaMigration;

/// Trait d'accès aux données blob pour la migration.
pub trait BlobNodeLocator {
    fn node_of(&self, id: BlobId) -> Option<usize>;
    fn byte_size(&self, id: BlobId) -> Option<u64>;
    fn move_to_node(&self, id: BlobId, target: usize) -> Result<(), FsError>;
}

impl NumaMigration {
    /// Migre un blob vers un nœud NUMA cible.
    pub fn migrate_blob(
        locator: &dyn BlobNodeLocator,
        id: BlobId,
        target_node: usize,
    ) -> MigrationResult {
        let from_node = match locator.node_of(id) {
            Some(n) => n,
            None    => return MigrationResult::NotFound,
        };
        if from_node == target_node {
            return MigrationResult::AlreadyOnTarget;
        }
        let bytes = locator.byte_size(id).unwrap_or(0);
        match locator.move_to_node(id, target_node) {
            Ok(()) => {
                NUMA_STATS.record_free(from_node, bytes);
                NUMA_STATS.record_alloc(target_node, bytes);
                NUMA_STATS.record_migration(from_node);
                MigrationResult::Migrated { from: from_node, to: target_node, bytes }
            }
            Err(e) => MigrationResult::Error(e),
        }
    }
}
