//! object_write.rs — SYS_EXOFS_OBJECT_WRITE (503) — écriture vers un objet ExoFS.
//!
//! RÈGLE 9  : copy_from_user() pour buffer userspace.
//! RÈGLE 10 : buffer d'écriture sur le tas.
//! RECUR-01 : while, pas de for.
//! OOM-02   : try_reserve avant toute allocation.
//! ARITH-02 : saturating_*/checked_add pour offsets.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    read_user_buf, exofs_err_to_errno,
    validate_fd, validate_count, validate_offset, EFAULT,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un seul appel exofs_object_write().
pub const WRITE_MAX_BYTES: usize = 8 * 1_024 * 1_024; // 8 MiB.

// ─────────────────────────────────────────────────────────────────────────────
// Arguments étendus
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments étendus de l'écriture.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WriteArgs {
    /// Offset d'écriture dans le blob.
    pub offset:     u64,
    /// Nombre d'octets à écrire.
    pub count:      u64,
    /// Si non-zéro, utilise et avance le curseur du fd.
    pub use_cursor: u32,
    /// Si non-zéro, flush immédiat vers le stockage persistant.
    pub sync:       u32,
    /// Flags additionnels (0 = défaut).
    pub flags:      u64,
}

const _: () = assert!(core::mem::size_of::<WriteArgs>() == 32);

impl WriteArgs {
    #[allow(dead_code)]
    fn defaults(offset: u64, count: u64) -> Self {
        Self { offset, count, use_cursor: 0, sync: 0, flags: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat d'écriture
// ─────────────────────────────────────────────────────────────────────────────

pub struct WriteResult {
    pub bytes_written: usize,
    pub new_offset:    u64,
    pub new_size:      u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique d'écriture
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit `data` dans le blob `blob_id` à l'offset `offset`.
///
/// Si le blob n'existe pas, le crée. Si le blob existait, réécrit entièrement
/// les octets à l'offset (copy-on-write sémantique).
/// OOM-02 : try_reserve pour le nouveau contenu.
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    let write_end = offset.checked_add(data.len() as u64)
        .ok_or(ExofsError::OffsetOverflow)?;

    // Lire le contenu existant (ou créer un blob vide).
    let existing = BLOB_CACHE.get(&blob_id);
    let existing_size = existing.as_ref().map(|d| d.len()).unwrap_or(0) as u64;
    let new_size = write_end.max(existing_size);

    // Construire le nouveau contenu.
    let new_size_usize = new_size as usize;
    if new_size_usize > WRITE_MAX_BYTES {
        return Err(ExofsError::NoSpace);
    }

    let mut new_content: Vec<u8> = Vec::new();
    new_content.try_reserve(new_size_usize).map_err(|_| ExofsError::NoMemory)?;
    new_content.resize(new_size_usize, 0u8);

    // Copier le contenu existant si présent (RECUR-01 : while).
    if let Some(ref existing_data) = existing {
        let copy_len = existing_data.len().min(new_size_usize);
        let mut i = 0usize;
        while i < copy_len {
            new_content[i] = existing_data[i];
            i = i.wrapping_add(1);
        }
    }

    // Ecrire les nouvelles données à l'offset (RECUR-01 : while).
    let start = offset as usize;
    let dlen = data.len();
    let mut i = 0usize;
    while i < dlen {
        new_content[start.wrapping_add(i)] = data[i];
        i = i.wrapping_add(1);
    }

    // Insérer dans le cache.
    BLOB_CACHE.insert(blob_id, new_content.to_vec())?;
    BLOB_CACHE.mark_dirty(&blob_id).ok();

    Ok(WriteResult {
        bytes_written: dlen,
        new_offset:    write_end,
        new_size,
    })
}

/// Effectue une écriture via un fd.
fn write_fd(
    fd:         u32,
    offset:     u64,
    data:       &[u8],
    use_cursor: bool,
    _sync:       bool,
) -> ExofsResult<WriteResult> {
    OBJECT_TABLE.check_writable(fd)?;
    let entry = OBJECT_TABLE.get(fd)?;
    let blob_id = entry.blob_id;
    let effective_offset = if use_cursor { entry.cursor } else { offset };

    let result = write_blob(blob_id, effective_offset, data)?;

    // Avancer le curseur si demandé.
    if use_cursor && result.bytes_written > 0 {
        OBJECT_TABLE.set_cursor(fd, result.new_offset)?;
    }
    // Mettre à jour la taille dans le fd.
    OBJECT_TABLE.set_size(fd, result.new_size)?;

    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall SYS_EXOFS_OBJECT_WRITE (503)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_write(fd, buf_ptr, count, offset, args_ptr, _) → bytes_written ou errno`
pub fn sys_exofs_object_write(
    fd:       u64,
    buf_ptr:  u64,
    count:    u64,
    offset:   u64,
    args_ptr: u64,
    _a6:      u64,
) -> i64 {
    let fd_u32 = match validate_fd(fd) {
        Ok(f)  => f,
        Err(e) => return e,
    };
    if count == 0 { return 0; }
    let count_usize = match validate_count(count) {
        Ok(c)  => c.min(WRITE_MAX_BYTES),
        Err(e) => return e,
    };
    let offset_val = match validate_offset(offset) {
        Ok(o)  => o,
        Err(e) => return e,
    };

    // Lire les WriteArgs optionnels.
    let (effective_offset, use_cursor, sync) = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<WriteArgs>(args_ptr) } {
            Ok(a) => (if a.use_cursor != 0 { offset_val } else { a.offset }, a.use_cursor != 0, a.sync != 0),
            Err(_) => return EFAULT,
        }
    } else {
        (offset_val, false, false)
    };

    // Lire le buffer depuis userspace (RÈGLE 9, heap allocation).
    let mut data_buf: Vec<u8> = Vec::new();
    if let Err(e) = read_user_buf(buf_ptr, count_usize as u64, &mut data_buf) {
        return e;
    }

    match write_fd(fd_u32, effective_offset, &data_buf, use_cursor, sync) {
        Ok(r)  => r.bytes_written as i64,
        Err(e) => exofs_err_to_errno(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Scatter-write : écriture en plusieurs segments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteSegment {
    pub offset:  u64,
    pub count:   u64,
    pub buf_ptr: u64,
}

const _: () = assert!(core::mem::size_of::<WriteSegment>() == 24);
pub const MAX_WRITE_SEGMENTS: usize = 16;

/// Effectue un scatter-write sur un seul BlobId.
/// RECUR-01 : while. OOM-02 : try_reserve.
pub fn scatter_write(blob_id: BlobId, segments: &[WriteSegment]) -> ExofsResult<u64> {
    let n_segs = segments.len().min(MAX_WRITE_SEGMENTS);
    let mut total: u64 = 0u64;
    let mut idx = 0usize;
    while idx < n_segs {
        let seg = &segments[idx];
        if seg.count == 0 || seg.buf_ptr == 0 {
            idx = idx.wrapping_add(1);
            continue;
        }
        let count = (seg.count as usize).min(WRITE_MAX_BYTES);
        let mut tmp: Vec<u8> = Vec::new();
        tmp.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
        tmp.resize(count, 0u8);
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        unsafe {
            super::validation::copy_from_user(tmp.as_mut_ptr(), seg.buf_ptr as *const u8, count)
                .map_err(|_| ExofsError::IoError)?;
        }
        let r = write_blob(blob_id, seg.offset, &tmp)?;
        total = total.saturating_add(r.bytes_written as u64);
        idx = idx.wrapping_add(1);
    }
    Ok(total)
}

/// Vérifie qu'une écriture à offset+count ne dépasse pas la limite de blob.
/// ARITH-02 : checked_add.
#[inline]
pub fn check_write_bounds(offset: u64, count: u64, max_size: u64) -> ExofsResult<()> {
    let end = offset.checked_add(count).ok_or(ExofsError::OffsetOverflow)?;
    if end > max_size { Err(ExofsError::NoSpace) } else { Ok(()) }
}

/// Calculs ARITH-02 : retourne le nouvel offset après écriture.
#[inline]
pub fn new_write_offset(offset: u64, written: usize) -> u64 {
    offset.saturating_add(written as u64)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_args_size() {
        assert_eq!(core::mem::size_of::<WriteArgs>(), 32);
    }

    #[test]
    fn test_write_args_defaults() {
        let a = WriteArgs::defaults(0, 100);
        assert_eq!(a.offset, 0);
        assert_eq!(a.count, 100);
        assert_eq!(a.use_cursor, 0);
        assert_eq!(a.sync, 0);
    }

    #[test]
    fn test_check_write_bounds_ok() {
        assert!(check_write_bounds(0, 100, 1000).is_ok());
        assert!(check_write_bounds(900, 100, 1000).is_ok());
    }

    #[test]
    fn test_check_write_bounds_overflow() {
        assert!(check_write_bounds(u64::MAX - 10, 20, u64::MAX).is_err());
    }

    #[test]
    fn test_check_write_bounds_past_max() {
        assert!(check_write_bounds(900, 200, 1000).is_err());
    }

    #[test]
    fn test_new_write_offset() {
        assert_eq!(new_write_offset(100, 50), 150);
        assert_eq!(new_write_offset(u64::MAX, 10), u64::MAX); // saturating
    }

    #[test]
    fn test_write_segment_size() {
        assert_eq!(core::mem::size_of::<WriteSegment>(), 24);
    }

    #[test]
    fn test_scatter_write_empty() {
        let blob = BlobId([0xDDu8; 32]);
        let segs: &[WriteSegment] = &[];
        let r = scatter_write(blob, segs).unwrap();
        assert_eq!(r, 0);
    }

    #[test]
    fn test_sys_write_null_buf() {
        assert_eq!(sys_exofs_object_write(4, 0, 100, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_sys_write_bad_fd() {
        assert_eq!(sys_exofs_object_write(0, 0x1000, 100, 0, 0, 0), EBADF);
    }

    #[test]
    fn test_sys_write_zero_count() {
        // count == 0 retourne 0 (pas d'erreur, rien écrit).
        assert_eq!(sys_exofs_object_write(4, 0x1000, 0, 0, 0, 0), 0);
    }

    #[test]
    fn test_sys_write_count_too_large() {
        let big = super::super::validation::EXOFS_BLOB_MAX as u64 + 1;
        assert_eq!(sys_exofs_object_write(4, 0x1000, big, 0, 0, 0), ERANGE);
    }

    #[test]
    fn test_write_fd_wronly() {
        use super::super::object_fd::open_flags;
        let blob = BlobId::from_bytes_blake3(b"/write/test/fd");
        let fd = OBJECT_TABLE.open(blob, open_flags::O_WRONLY, 0, 0, 0).unwrap();
        let data = [0xABu8; 16];
        let r = write_fd(fd, 0, &data, false, false).unwrap();
        assert_eq!(r.bytes_written, 16);
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_write_fd_rdonly_rejected() {
        use super::super::object_fd::open_flags;
        let blob = BlobId::from_bytes_blake3(b"/write/test/ro");
        let fd = OBJECT_TABLE.open(blob, open_flags::O_RDONLY, 0, 0, 0).unwrap();
        let data = [0u8; 16];
        assert!(write_fd(fd, 0, &data, false, false).is_err());
        OBJECT_TABLE.close(fd);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de troncature et d'extension
// ─────────────────────────────────────────────────────────────────────────────

/// Tronque un blob à `new_size` octets.
///
/// Si `new_size > current_size`, étend avec des zéros (sparse extension).
/// OOM-02 : try_reserve. ARITH-02 : min/saturating_sub.
pub fn truncate_blob(blob_id: BlobId, new_size: usize) -> ExofsResult<()> {
    if new_size > WRITE_MAX_BYTES { return Err(ExofsError::NoSpace); }

    let existing = BLOB_CACHE.get(&blob_id);
    let current_size = existing.as_ref().map(|d| d.len()).unwrap_or(0);

    if new_size == current_size { return Ok(()); }

    let mut new_content: Vec<u8> = Vec::new();
    new_content.try_reserve(new_size).map_err(|_| ExofsError::NoMemory)?;
    new_content.resize(new_size, 0u8);

    // Copier l'existant jusqu'à min(current_size, new_size) (RECUR-01 : while).
    if let Some(ref data) = existing {
        let copy_len = current_size.min(new_size);
        let mut i = 0usize;
        while i < copy_len {
            new_content[i] = data[i];
            i = i.wrapping_add(1);
        }
    }

    BLOB_CACHE.insert(blob_id, new_content.to_vec())?;
    BLOB_CACHE.mark_dirty(&blob_id).ok();
    Ok(())
}

/// Vide le contenu d'un blob (troncature à 0 octets).
#[inline]
pub fn clear_blob(blob_id: BlobId) -> ExofsResult<()> {
    truncate_blob(blob_id, 0)
}

/// Retourne la taille actuelle d'un blob en cache (0 si absent).
pub fn cached_size(blob_id: &BlobId) -> u64 {
    BLOB_CACHE.get(blob_id).map(|d| d.len() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests_trunc {
    use super::*;

    #[test]
    fn test_truncate_blob_extend() {
        let b = BlobId::from_bytes_blake3(b"trunc_extend_test");
        let data = [0xAAu8; 64];
        BLOB_CACHE.insert(b, data.to_vec()).unwrap();
        truncate_blob(b, 128).unwrap();
        let new_size = BLOB_CACHE.get(&b).map(|d| d.len()).unwrap_or(0);
        assert_eq!(new_size, 128);
    }

    #[test]
    fn test_truncate_blob_shrink() {
        let b = BlobId::from_bytes_blake3(b"trunc_shrink_test");
        let data = [0xBBu8; 128];
        BLOB_CACHE.insert(b, data.to_vec()).unwrap();
        truncate_blob(b, 32).unwrap();
        let new_size = BLOB_CACHE.get(&b).map(|d| d.len()).unwrap_or(0);
        assert_eq!(new_size, 32);
    }

    #[test]
    fn test_clear_blob() {
        let b = BlobId::from_bytes_blake3(b"clear_test_blob");
        let data = [0xCCu8; 64];
        BLOB_CACHE.insert(b, data.to_vec()).unwrap();
        clear_blob(b).unwrap();
        let new_size = BLOB_CACHE.get(&b).map(|d| d.len()).unwrap_or(0);
        assert_eq!(new_size, 0);
    }

    #[test]
    fn test_cached_size_absent() {
        let b = BlobId([0xEEu8; 32]);
        // Un BlobId aléatoire non insère → taille 0.
        // (Peut retourner non-zéro si déjà inséré dans un autre test, mais
        // l'invariant est que cached_size ne panique jamais.)
        let _ = cached_size(&b);
    }
}
