//! SnapshotRestore — restauration d'un snapshot ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::snapshot::{Snapshot, SnapshotId};
use super::snapshot_list::SNAPSHOT_LIST;

/// Résultat d'une restauration.
#[derive(Clone, Debug)]
pub struct RestoreResult {
    pub snap_id:      SnapshotId,
    pub root_blob:    BlobId,
    pub n_blobs:      u64,
    pub total_bytes:  u64,
}

/// Interface d'accès aux blobs à restaurer.
pub trait RestoreSink: Send + Sync {
    /// Reçoit un blob à écrire / rétablir.
    fn write_blob(&mut self, blob_id: BlobId, data: &[u8]) -> Result<(), FsError>;
    /// Appelé quand la restauration est terminée.
    fn finalize(&mut self) -> Result<(), FsError>;
}

/// Interface de lecture des blobs d'un snapshot.
pub trait SnapshotBlobSource: Send + Sync {
    fn read_blob(&self, snap_id: SnapshotId, blob_id: BlobId) -> Result<Vec<u8>, FsError>;
    fn list_blobs(&self, snap_id: SnapshotId) -> Result<Vec<BlobId>, FsError>;
}

pub struct SnapshotRestore;

impl SnapshotRestore {
    /// Restaure un snapshot vers `sink`.
    pub fn restore(
        snap_id: SnapshotId,
        source:  &dyn SnapshotBlobSource,
        sink:    &mut dyn RestoreSink,
    ) -> Result<RestoreResult, FsError> {
        let snap = SNAPSHOT_LIST.get(snap_id).ok_or(FsError::NotFound)?;
        let blobs = source.list_blobs(snap_id)?;

        let mut total_bytes = 0u64;
        for &blob_id in &blobs {
            let data = source.read_blob(snap_id, blob_id)?;
            total_bytes = total_bytes.checked_add(data.len() as u64).ok_or(FsError::Overflow)?;
            sink.write_blob(blob_id, &data)?;
        }
        sink.finalize()?;

        Ok(RestoreResult {
            snap_id,
            root_blob:   snap.root_blob,
            n_blobs:     blobs.len() as u64,
            total_bytes,
        })
    }
}
