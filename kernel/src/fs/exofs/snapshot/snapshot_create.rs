//! SnapshotCreator — création de snapshots ExoFS (no_std).
//! RÈGLE 11 : BlobId = Blake3(données brutes AVANT compression/chiffrement).

use alloc::vec::Vec;
use crate::arch::time::read_ticks;
use crate::fs::exofs::core::{BlobId, EpochId, FsError};
use super::snapshot::{Snapshot, SnapshotId, flags};
use super::snapshot_list::SNAPSHOT_LIST;

/// Paramètres de création d'un snapshot.
pub struct SnapshotParams {
    pub name:      [u8; 64],
    pub parent_id: Option<SnapshotId>,
    pub flags:     u32,
}

impl SnapshotParams {
    pub fn new_readonly(name: &[u8]) -> Self {
        let mut arr = [0u8; 64];
        let n = name.len().min(64);
        arr[..n].copy_from_slice(&name[..n]);
        Self { name: arr, parent_id: None, flags: flags::READONLY }
    }

    pub fn new_protected(name: &[u8]) -> Self {
        let mut arr = [0u8; 64];
        let n = name.len().min(64);
        arr[..n].copy_from_slice(&name[..n]);
        Self { name: arr, parent_id: None, flags: flags::PROTECTED | flags::READONLY }
    }
}

pub struct SnapshotCreator;

impl SnapshotCreator {
    /// Crée un nouveau snapshot — l'appelant fournit la liste des blobs.
    ///
    /// Le root_blob est calculé comme Blake3 de la concaténation des BlobIds
    /// (RÈGLE 11 : au moment de la capture, avant toute transformation).
    pub fn create(
        epoch_id:   EpochId,
        params:     SnapshotParams,
        blobs:      &[BlobId],
    ) -> Result<SnapshotId, FsError> {
        // Calcul du root_blob = Blake3(concaténation des raw blob ids).
        let root_blob = Self::compute_root_blob(blobs)?;

        let id = SNAPSHOT_LIST.allocate_id();
        let snapshot = Snapshot {
            id,
            epoch_id,
            parent_id:   params.parent_id,
            root_blob,
            created_at:  read_ticks(),
            n_blobs:     blobs.len() as u64,
            total_bytes: 0,   // Le montant exact est mis à jour par le caller.
            flags:       params.flags,
            name:        params.name,
        };

        SNAPSHOT_LIST.register(snapshot)?;
        Ok(id)
    }

    /// Calcul RÈGLE 11 : root_blob = Blake3(tous les blob ids concaténés).
    fn compute_root_blob(blobs: &[BlobId]) -> Result<BlobId, FsError> {
        // Construire le buffer sur le tas (RÈGLE 10 : pas sur stack).
        let n_bytes = blobs.len()
            .checked_mul(32)
            .ok_or(FsError::Overflow)?;
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(n_bytes).map_err(|_| FsError::OutOfMemory)?;
        for blob in blobs {
            for b in blob.as_bytes().iter() {
                buf.push(*b);
            }
        }
        Ok(BlobId::from_bytes_blake3(&buf))
    }
}
