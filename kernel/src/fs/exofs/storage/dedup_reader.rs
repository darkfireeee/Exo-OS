//! dedup_reader.rs — Lecture d'un blob dédupliqué depuis le storage ExoFS (no_std).
//! RÈGLE 11 : Vérification BlobId après lecture.

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::blob_reader::{BlobReadResult, read_blob};

/// Résultat de lecture dédupliquée.
#[derive(Clone, Debug)]
pub struct DedupReadResult {
    pub blob_id:   BlobId,
    pub data:      Vec<u8>,
    pub verified:  bool,
}

/// Lit un blob et vérifie son intégrité via BlobId (RÈGLE 11).
pub fn read_dedup_blob(
    heap_offset: u64,
    expected_id: BlobId,
    buf:         &[u8],
) -> Result<DedupReadResult, FsError> {
    let read_res = read_blob(buf, heap_offset)?;

    // RÈGLE 11 : vérifier que BlobId(données) == expected_id.
    let computed_id = BlobId::from_bytes_blake3(&read_res.data);
    if computed_id.as_bytes() != expected_id.as_bytes() {
        return Err(FsError::IntegrityCheckFailed);
    }

    Ok(DedupReadResult {
        blob_id:  computed_id,
        data:     read_res.data,
        verified: true,
    })
}
