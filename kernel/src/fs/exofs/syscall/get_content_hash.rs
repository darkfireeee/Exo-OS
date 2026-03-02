//! SYS_EXOFS_GET_CONTENT_HASH (508) — retourne le hash de contenu d'un objet ExoFS.
//! RÈGLE 9 : copy_to_user() pour l'ID de sortie.
//! Audité SEC-09 (hash cryptographique).

use super::validation::{write_user_buf, fserr_to_errno, EFAULT, EINVAL};
use super::object_fd::OBJECT_TABLE;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;

/// `exofs_get_content_hash(fd, hash_ptr, hash_len) -> 0 ou errno`
///
/// Écrit le hash Blake3 (32 bytes) du contenu de l'objet vers `hash_ptr`.
pub fn sys_exofs_get_content_hash(
    fd:       u64,
    hash_ptr: u64,
    hash_len: u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if hash_ptr == 0 { return EFAULT; }
    if hash_len < 32 { return EINVAL; }

    let blob_id = match OBJECT_TABLE.get_blob_id(fd as u32) {
        Some(b) => b,
        None    => return super::validation::ENOENT,
    };

    // Lire depuis le cache.
    if let Some(data) = BLOB_CACHE.get(&blob_id) {
        // RÈGLE 11 : BlobId = Blake3(données brutes AVANT compression/chiffrement).
        // Le blob_id est déjà calculé sur les données originales.
        let hash = blob_id.as_bytes();
        if let Err(e) = write_user_buf(hash_ptr, &hash) {
            return e;
        }
        0
    } else {
        // Si l'objet n'est pas en cache, retourner le BlobId connu.
        let hash = blob_id.as_bytes();
        if let Err(e) = write_user_buf(hash_ptr, &hash) {
            return e;
        }
        0
    }
}
