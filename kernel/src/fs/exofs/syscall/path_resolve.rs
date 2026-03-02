//! SYS_EXOFS_PATH_RESOLVE (500) — résolution de chemin ExoFS vers ObjectId.
//! RÈGLE 9 : copy_from_user() obligatoire.
//! RÈGLE 10 : buffer PATH_MAX sur le tas uniquement.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use crate::fs::exofs::path::resolver::PathResolver;
use super::validation::{read_user_path_heap, write_user_buf, fserr_to_errno, EFAULT, EINVAL};

/// `exofs_path_resolve(path_ptr, path_len, out_id_ptr) -> object_id_lo as i64`
///
/// Résout `path_ptr` en un ObjectId (u64). Écrit 8 octets le/u64 dans `out_id_ptr`.
/// Retourne l'id (positif) ou errno (négatif).
pub fn sys_exofs_path_resolve(
    path_ptr:   u64,
    _path_len:  u64,
    out_ptr:    u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    // RÈGLE 10 : buffer heap.
    let mut path_buf: Vec<u8> = Vec::new();
    let len = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };

    let path_str = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s)  => s,
        Err(_) => return EINVAL,
    };

    let object_id = match PathResolver::resolve(path_str) {
        Ok(id) => id,
        Err(e) => return fserr_to_errno(e),
    };

    // Écrire l'id vers userspace si out_ptr non nul.
    if out_ptr != 0 {
        let id_bytes = object_id.to_le_bytes();
        if let Err(e) = write_user_buf(out_ptr, &id_bytes) {
            return e;
        }
    }

    object_id as i64
}
