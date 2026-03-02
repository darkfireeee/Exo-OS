//! SnapshotDeleter — suppression de snapshots ExoFS (no_std).

use crate::fs::exofs::core::FsError;
use super::snapshot::SnapshotId;
use super::snapshot_list::SNAPSHOT_LIST;

/// Résultat de suppression.
#[derive(Debug, Clone, Copy)]
pub enum DeleteResult {
    Deleted,
    NotFound,
    Protected,
}

pub struct SnapshotDeleter;

impl SnapshotDeleter {
    /// Supprime un snapshot. Retourne une erreur si protégé.
    pub fn delete(id: SnapshotId) -> Result<DeleteResult, FsError> {
        let snap = match SNAPSHOT_LIST.get(id) {
            Some(s) => s,
            None    => return Ok(DeleteResult::NotFound),
        };

        if snap.is_protected() {
            return Ok(DeleteResult::Protected);
        }

        // Vérifier qu'aucun autre snapshot n'a ce snapshot pour parent.
        let child_count = Self::count_children(id);
        if child_count > 0 {
            // Ne pas supprimer un snapshot qui a des enfants vivants.
            return Err(FsError::Busy);
        }

        let removed = SNAPSHOT_LIST.remove(id);
        if removed {
            Ok(DeleteResult::Deleted)
        } else {
            Ok(DeleteResult::NotFound)
        }
    }

    /// Suppression forcée (ignore la protection — accès kernel interne).
    pub fn force_delete(id: SnapshotId) -> bool {
        SNAPSHOT_LIST.remove(id)
    }

    fn count_children(parent: SnapshotId) -> usize {
        SNAPSHOT_LIST.all_ids().iter().filter(|&&sid| {
            SNAPSHOT_LIST.get(sid)
                .and_then(|s| s.parent_id)
                .map_or(false, |p| p == parent)
        }).count()
    }
}
