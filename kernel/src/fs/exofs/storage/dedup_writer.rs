//! dedup_writer.rs — Écriture dédupliquée de blobs dans le storage ExoFS (no_std).
//! RÈGLE 11 : BlobId = Blake3(données brutes AVANT compression/chiffrement).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::dedup::dedup_api::DedupApi;
use super::blob_writer::{BlobHeader, BlobWriteResult, write_blob};

/// Résultat d'écriture dédupliquée.
#[derive(Clone, Debug)]
pub struct DedupWriteResult {
    pub blob_id:       BlobId,
    pub was_dedup:     bool,
    pub write_result:  Option<BlobWriteResult>,   // None si dédup (pas d'écriture physique).
    pub chunks_dedup:  u32,
    pub bytes_saved:   u64,
}

/// Écrit un blob en appliquant la déduplication.
///
/// RÈGLE 11 : BlobId calculé sur `data` brute avant toute transformation.
pub fn write_dedup_blob(
    data:     &[u8],
    inode_id: u64,
    heap_offset_fn: &mut dyn FnMut(u64) -> Result<u64, FsError>,
) -> Result<DedupWriteResult, FsError> {
    // RÈGLE 11 : BlobId des données brutes EN PREMIER.
    let blob_id = BlobId::from_bytes_blake3(data);

    // Tenter la déduplication.
    let dedup_result = DedupApi::dedup_blob(data, inode_id)?;

    if dedup_result.was_dedup {
        return Ok(DedupWriteResult {
            blob_id:      dedup_result.blob_id,
            was_dedup:    true,
            write_result: None,
            chunks_dedup: dedup_result.n_chunks as u32,
            bytes_saved:  data.len() as u64,
        });
    }

    // Pas de dédup → écriture physique.
    let heap_offset = heap_offset_fn(data.len() as u64)?;
    let write_res   = write_blob(data, heap_offset)?;

    Ok(DedupWriteResult {
        blob_id:      write_res.blob_id,
        was_dedup:    false,
        write_result: Some(write_res),
        chunks_dedup: 0,
        bytes_saved:  0,
    })
}
