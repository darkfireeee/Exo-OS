//! SnapshotDiff — comparaison de deux snapshots ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::snapshot::{Snapshot, SnapshotId};
use super::snapshot_list::SNAPSHOT_LIST;

/// Type de différence entre deux snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Added,    // Présent dans B, absent dans A.
    Removed,  // Présent dans A, absent dans B.
    Changed,  // Présent dans les deux mais root_blob différent.
}

/// Entrée d'un diff de snapshot.
#[derive(Clone, Debug)]
pub struct DiffEntry {
    pub kind:    DiffKind,
    pub blob_id: BlobId,
}

/// Rapport de diff entre deux snapshots.
#[derive(Clone, Debug)]
pub struct SnapshotDiffReport {
    pub snap_a:      SnapshotId,
    pub snap_b:      SnapshotId,
    pub n_added:     u32,
    pub n_removed:   u32,
    pub n_changed:   u32,
    pub entries:     Vec<DiffEntry>,
}

/// Interface d'énumération des blobs d'un snapshot.
pub trait SnapshotBlobEnumerator: Send + Sync {
    fn list_blobs(&self, snap_id: SnapshotId) -> Result<Vec<BlobId>, FsError>;
}

pub struct SnapshotDiff;

impl SnapshotDiff {
    pub fn compute(
        a: SnapshotId,
        b: SnapshotId,
        enumerator: &dyn SnapshotBlobEnumerator,
    ) -> Result<SnapshotDiffReport, FsError> {
        // Vérifier existence.
        let _snap_a = SNAPSHOT_LIST.get(a).ok_or(FsError::NotFound)?;
        let _snap_b = SNAPSHOT_LIST.get(b).ok_or(FsError::NotFound)?;

        let blobs_a = enumerator.list_blobs(a)?;
        let blobs_b = enumerator.list_blobs(b)?;

        let mut entries = Vec::new();

        // Blobs ajoutés (dans b, pas dans a).
        for &blob in &blobs_b {
            if !blobs_a.iter().any(|x| x.as_bytes() == blob.as_bytes()) {
                entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                entries.push(DiffEntry { kind: DiffKind::Added, blob_id: blob });
            }
        }

        // Blobs supprimés (dans a, pas dans b).
        for &blob in &blobs_a {
            if !blobs_b.iter().any(|x| x.as_bytes() == blob.as_bytes()) {
                entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                entries.push(DiffEntry { kind: DiffKind::Removed, blob_id: blob });
            }
        }

        let n_added   = entries.iter().filter(|e| e.kind == DiffKind::Added).count() as u32;
        let n_removed = entries.iter().filter(|e| e.kind == DiffKind::Removed).count() as u32;
        let n_changed = entries.iter().filter(|e| e.kind == DiffKind::Changed).count() as u32;

        Ok(SnapshotDiffReport { snap_a: a, snap_b: b, n_added, n_removed, n_changed, entries })
    }
}
