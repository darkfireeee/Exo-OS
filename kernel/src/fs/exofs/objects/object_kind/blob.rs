// kernel/src/fs/exofs/objects/object_kind/blob.rs
//
// Opérations spécifiques aux objets Blob (contenu binaire immutable).
// Blob == Class1 + BlobId == ContentHash.

use crate::fs::exofs::core::{BlobId, ExofsResult};

/// Vérifie qu'un BlobId correspond aux données attendues.
pub fn blob_verify_content(data: &[u8], expected_blob_id: &BlobId) -> bool {
    crate::fs::exofs::core::verify_blob_id(data, expected_blob_id)
}
