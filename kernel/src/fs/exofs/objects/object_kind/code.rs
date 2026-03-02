// kernel/src/fs/exofs/objects/object_kind/code.rs
// Objets Code — exécutables ou bibliothèques vérifiés par BlobId.
use crate::fs::exofs::core::BlobId;
/// Vérifie qu'un objet Code est valide (BlobId cohérent).
pub fn code_is_valid(data: &[u8], blob_id: &BlobId) -> bool {
    crate::fs::exofs::core::verify_blob_id(data, blob_id)
}
