//! object_delete.rs — SYS_EXOFS_OBJECT_DELETE (505) — suppression d'un objet ExoFS.
//!
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    read_user_path_heap, exofs_err_to_errno, EFAULT, EINVAL,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Flags de suppression.
pub mod delete_flags {
    /// Forcer la suppression même si des fd sont ouverts (dangereux).
    pub const FORCE:       u32 = 0x0001;
    /// Supprimer les liens symboliques sans suivre leur cible.
    pub const NO_FOLLOW:   u32 = 0x0002;
    /// Retourner OK si l'objet n'existe déjà plus (idempotent).
    pub const IDEMPOTENT:  u32 = 0x0004;
    /// Supprimer récursivement (répertoires).
    pub const RECURSIVE:   u32 = 0x0008;
    /// Masque de flags valides.
    pub const VALID_MASK:  u32 = FORCE | NO_FOLLOW | IDEMPOTENT | RECURSIVE;
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une suppression.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DeleteResult {
    pub blob_id:     [u8; 32],
    pub bytes_freed: u64,
    pub tombstoned:  u32,
    pub _pad:        u32,
}

const _: () = assert!(core::mem::size_of::<DeleteResult>() == 48);

// ─────────────────────────────────────────────────────────────────────────────
// Logique interne
// ─────────────────────────────────────────────────────────────────────────────

/// Valide les flags de suppression.
fn validate_delete_flags(flags: u32) -> ExofsResult<()> {
    if flags & !delete_flags::VALID_MASK != 0 {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(())
}

/// Supprime un objet identifié par son BlobId.
///
/// - Vérifie d'abord l'existence.
/// - Refuse la suppression si des fd sont ouverts (sauf FORCE).
/// - Invalide le cache.
fn delete_blob(blob_id: BlobId, flags: u32) -> ExofsResult<DeleteResult> {
    validate_delete_flags(flags)?;

    // Vérifier l'existence.
    let existing = BLOB_CACHE.get(&blob_id);
    let bytes_freed = match &existing {
        Some(data) => data.len() as u64,
        None => {
            if flags & delete_flags::IDEMPOTENT != 0 {
                return Ok(DeleteResult {
                    blob_id:     *blob_id.as_bytes(),
                    bytes_freed: 0,
                    tombstoned:  0,
                    _pad:        0,
                });
            }
            return Err(ExofsError::BlobNotFound);
        }
    };
    drop(existing);

    // Refuser si des fd sont ouverts et que FORCE est absent.
    let open = OBJECT_TABLE.open_count();
    if open > 0 && flags & delete_flags::FORCE == 0 {
        // Vérifier si *ce* blob est ouvert.
        // On parcourt en utilisant une clef de présence via open_count.
        // En l'absence d'API directe, on se base sur l'existence d'un fd portant ce blob_id.
        let has_open_fd = object_has_open_fd(&blob_id);
        if has_open_fd {
            return Err(ExofsError::PermissionDenied);
        }
    }

    BLOB_CACHE.invalidate(&blob_id);

    Ok(DeleteResult {
        blob_id:     *blob_id.as_bytes(),
        bytes_freed,
        tombstoned:  1,
        _pad:        0,
    })
}

/// Vérifie si un blob donné possède au moins un fd ouvert dans OBJECT_TABLE.
/// RECUR-01 : while, no for.
fn object_has_open_fd(blob_id: &BlobId) -> bool {
    let target = blob_id.as_bytes();
    // On tente FD_RESERVED..=FD_MAX de façon linéaire.
    let start = super::object_fd::FD_RESERVED as u32;
    let end   = super::object_fd::FD_MAX   as u32;
    let mut fd = start;
    while fd <= end {
        if let Ok(entry) = OBJECT_TABLE.get(fd) {
            let bid = entry.blob_id.as_bytes();
            let mut eq = true;
            let mut i = 0usize;
            while i < 32 {
                if bid[i] != target[i] { eq = false; break; }
                i = i.wrapping_add(1);
            }
            if eq { return true; }
        }
        fd = fd.saturating_add(1);
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Suppression par chemin
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime un objet à partir d'un chemin.
fn delete_object_by_path(
    path_bytes: &[u8],
    path_len:   usize,
    flags:      u32,
) -> ExofsResult<DeleteResult> {
    super::object_create::validate_create_path(path_bytes, path_len)?;
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);
    delete_blob(blob_id, flags)
}

// ─────────────────────────────────────────────────────────────────────────────
// Suppression par fd
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime l'objet associé au fd, puis ferme le fd.
pub fn delete_by_fd(fd: u32, flags: u32) -> ExofsResult<DeleteResult> {
    let blob_id = OBJECT_TABLE.blob_id_of(fd)?;
    // Fermer d'abord le fd pour éviter le conflit.
    OBJECT_TABLE.close(fd);
    let force_flags = flags | delete_flags::FORCE;
    delete_blob(blob_id, force_flags)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_OBJECT_DELETE (505)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_delete(path_ptr, path_len, flags, out_ptr, _, _) → 0 ou errno`
pub fn sys_exofs_object_delete(
    path_ptr: u64,
    path_len: u64,
    flags:    u64,
    out_ptr:  u64,
    _a5:      u64,
    _a6:      u64,
) -> i64 {
    if path_ptr == 0 { return EFAULT; }

    let mut path_buf: Vec<u8> = Vec::new();
    let actual_len = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };

    let result = match delete_object_by_path(&path_buf, actual_len, flags as u32) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };

    if out_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &result as *const DeleteResult as *const u8,
                core::mem::size_of::<DeleteResult>(),
            )
        };
        if let Err(e) = super::validation::write_user_buf(out_ptr, bytes) {
            return e;
        }
    }

    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nombre d'octets libérés lors de la suppression d'un blob.
/// Retourne 0 si le blob n'existe pas.
pub fn blob_size(blob_id: &BlobId) -> u64 {
    BLOB_CACHE.get(blob_id).map(|d| d.len() as u64).unwrap_or(0)
}

/// Suppression silencieuse (idempotente) : ne remonte jamais d'erreur.
pub fn silent_delete(blob_id: BlobId) {
    BLOB_CACHE.invalidate(&blob_id);
}

/// Retourne `true` si l'objet existe dans le cache.
pub fn object_exists(blob_id: &BlobId) -> bool {
    BLOB_CACHE.get(blob_id).is_some()
}

/// Supprime un batch de BlobIds.
/// OOM-02 : le vec de résultats est pré-réservé.
/// RECUR-01 : while.
pub fn batch_delete(ids: &[BlobId], flags: u32) -> ExofsResult<Vec<DeleteResult>> {
    let mut results: Vec<DeleteResult> = Vec::new();
    results.try_reserve(ids.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < ids.len() {
        let r = delete_blob(ids[i], flags).unwrap_or_default();
        results.push(r);
        i = i.wrapping_add(1);
    }
    Ok(results)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob(path: &[u8], data: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, data).unwrap();
        id
    }

    #[test]
    fn test_delete_existing() {
        let id = make_blob(b"/delete/existing", b"data");
        let r = delete_blob(id, 0).unwrap();
        assert_eq!(r.bytes_freed, 4);
        assert_eq!(r.tombstoned, 1);
    }

    #[test]
    fn test_delete_nonexistent_error() {
        let id = BlobId::from_bytes_blake3(b"/does/not/exist/xyz");
        assert!(delete_blob(id, 0).is_err());
    }

    #[test]
    fn test_delete_idempotent() {
        let id = BlobId::from_bytes_blake3(b"/idempotent/del");
        let r = delete_blob(id, delete_flags::IDEMPOTENT).unwrap();
        assert_eq!(r.bytes_freed, 0);
    }

    #[test]
    fn test_delete_by_path() {
        let path = b"/path/delete/obj";
        BLOB_CACHE.insert(BlobId::from_bytes_blake3(path), b"x".to_vec()).unwrap();
        let r = delete_object_by_path(path, path.len(), 0).unwrap();
        assert_eq!(r.tombstoned, 1);
    }

    #[test]
    fn test_object_exists_true() {
        let id = make_blob(b"/exists/check", b"y");
        assert!(object_exists(&id));
        silent_delete(id);
        assert!(!object_exists(&id));
    }

    #[test]
    fn test_blob_size() {
        let id = make_blob(b"/size/check", b"hello world");
        assert_eq!(blob_size(&id), 11);
        silent_delete(id);
        assert_eq!(blob_size(&id), 0);
    }

    #[test]
    fn test_validate_delete_flags_valid() {
        assert!(validate_delete_flags(delete_flags::FORCE | delete_flags::IDEMPOTENT).is_ok());
    }

    #[test]
    fn test_validate_delete_flags_invalid() {
        assert!(validate_delete_flags(0xDEAD).is_err());
    }

    #[test]
    fn test_batch_delete() {
        let ids = [
            make_blob(b"/batch/del/1", b"a"),
            make_blob(b"/batch/del/2", b"bb"),
            make_blob(b"/batch/del/3", b"ccc"),
        ];
        let results = batch_delete(&ids, delete_flags::FORCE).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].bytes_freed, 1);
        assert_eq!(results[1].bytes_freed, 2);
        assert_eq!(results[2].bytes_freed, 3);
    }

    #[test]
    fn test_delete_result_size() {
        assert_eq!(core::mem::size_of::<DeleteResult>(), 48);
    }

    #[test]
    fn test_sys_delete_null_path() {
        let r = sys_exofs_object_delete(0, 0, 0, 0, 0, 0);
        assert_eq!(r, EFAULT);
    }

    #[test]
    fn test_silent_delete_no_panic() {
        let id = BlobId::from_bytes_blake3(b"/silent/nonexistent");
        silent_delete(id); // ne doit pas paniquer
    }

    #[test]
    fn test_delete_already_deleted_idempotent() {
        let id = make_blob(b"/already/gone2", b"z");
        silent_delete(id);
        let r = delete_blob(id, delete_flags::IDEMPOTENT).unwrap();
        assert_eq!(r.bytes_freed, 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gestion avancée : tombstone log
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans le journal de tombstones — enregistre chaque suppression pour
/// compatibilité avec le garbage collector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TombstoneEntry {
    pub blob_id:    [u8; 32],
    pub epoch_id:   u64,
    pub deleted_at: u64,
    pub flags:      u32,
    pub _pad:       u32,
}

const _: () = assert!(core::mem::size_of::<TombstoneEntry>() == 56);

impl TombstoneEntry {
    /// Construit une entrée depuis un résultat de suppression.
    pub fn from_result(r: &DeleteResult, epoch: u64, ts: u64) -> Self {
        Self {
            blob_id:    r.blob_id,
            epoch_id:   epoch,
            deleted_at: ts,
            flags:      0,
            _pad:       0,
        }
    }
}

/// Encode un bloc de tombstones en octets pour transmission userspace.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn encode_tombstones(entries: &[TombstoneEntry]) -> ExofsResult<Vec<u8>> {
    let entry_size = core::mem::size_of::<TombstoneEntry>();
    let total = entries.len().saturating_mul(entry_size);
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < entries.len() {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let raw = unsafe {
            core::slice::from_raw_parts(
                &entries[i] as *const TombstoneEntry as *const u8,
                entry_size,
            )
        };
        let mut j = 0usize;
        while j < entry_size {
            buf.push(raw[j]);
            j = j.wrapping_add(1);
        }
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Récursivité simulée (répertoires) — RECUR-01 : while, pas de récursion
// ─────────────────────────────────────────────────────────────────────────────

/// Suppression d'un "répertoire" : on invalide le blob répertoire ainsi que
/// les entrées qu'il référence (stockées dans un format minimal header+entries).
///
/// Format répertoire : magic[4] + count[4] + entry_blob_id[32] * count.
/// RECUR-01 : while, pas de récursion.
pub fn delete_directory(dir_blob: BlobId, flags: u32) -> ExofsResult<u64> {
    let data = BLOB_CACHE.get(&dir_blob)
        .ok_or(ExofsError::BlobNotFound)?;

    let mut total_freed = 0u64;

    if data.len() >= 8 {
        let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let max_entries = (data.len().saturating_sub(8)) / 32;
        let n = count.min(max_entries);
        let mut i = 0usize;
        while i < n {
            let off = 8usize.saturating_add(i.saturating_mul(32));
            if off.saturating_add(32) > data.len() { break; }
            let mut id_bytes = [0u8; 32];
            let mut k = 0usize;
            while k < 32 {
                id_bytes[k] = data[off.saturating_add(k)];
                k = k.wrapping_add(1);
            }
            let child = BlobId(id_bytes);
            let child_size = blob_size(&child);
            BLOB_CACHE.invalidate(&child);
            total_freed = total_freed.saturating_add(child_size);
            i = i.wrapping_add(1);
        }
    }
    drop(data);
    let dir_size = blob_size(&dir_blob);
    BLOB_CACHE.invalidate(&dir_blob);
    Ok(total_freed.saturating_add(dir_size))
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn test_tombstone_entry_size() {
        assert_eq!(core::mem::size_of::<TombstoneEntry>(), 56);
    }

    #[test]
    fn test_tombstone_from_result() {
        let r = DeleteResult {
            blob_id:    [0xABu8; 32],
            bytes_freed: 100,
            tombstoned:  1,
            _pad:        0,
        };
        let t = TombstoneEntry::from_result(&r, 7, 12345);
        assert_eq!(t.epoch_id, 7);
        assert_eq!(t.deleted_at, 12345);
    }

    #[test]
    fn test_encode_tombstones_empty() {
        let buf = encode_tombstones(&[]).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn test_encode_tombstones_one() {
        let r = DeleteResult::default();
        let e = TombstoneEntry::from_result(&r, 1, 2);
        let buf = encode_tombstones(&[e]).unwrap();
        assert_eq!(buf.len(), 56);
    }

    #[test]
    fn test_encode_tombstones_multiple() {
        let entries = [TombstoneEntry::default(); 4];
        let buf = encode_tombstones(&entries).unwrap();
        assert_eq!(buf.len(), 56 * 4);
    }

    #[test]
    fn test_delete_directory_not_found() {
        let id = BlobId::from_bytes_blake3(b"/dir/nonex");
        assert!(delete_directory(id, 0).is_err());
    }

    #[test]
    fn test_delete_directory_empty() {
        let id = BlobId::from_bytes_blake3(b"/dir/empty/del");
        let mut hdr = [0u8; 8];
        hdr[0] = 0xCA; hdr[1] = 0xFE; hdr[2] = 0xD0; hdr[3] = 0xD1;
        BLOB_CACHE.insert(id, hdr.to_vec()).unwrap();
        let freed = delete_directory(id, 0).unwrap();
        assert_eq!(freed, 8);
        assert!(!object_exists(&id));
    }
}
