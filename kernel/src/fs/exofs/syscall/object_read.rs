//! object_read.rs — SYS_EXOFS_OBJECT_READ (502) — lecture d'un objet ExoFS.
//!
//! RÈGLE 9  : copy_to_user() pour écriture vers userspace.
//! RÈGLE 10 : buffer de lecture sur le tas.
//! RECUR-01 : while, pas de for.
//! OOM-02   : try_reserve avant toute allocation.
//! ARITH-02 : saturating_*/checked_div pour calculs d'offset.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    write_user_buf, exofs_err_to_errno,
    validate_fd, validate_count, validate_offset,
    EINVAL, EFAULT, ENOMEM, ERANGE, EBADF,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un seul appel exofs_object_read().
pub const READ_MAX_BYTES: usize = 8 * 1_024 * 1_024; // 8 MiB.
/// Taille du buffer de lecture alloué par défaut sur le tas.
pub const READ_BUF_DEFAULT: usize = 4_096; // 4 KiB, pour les petits reads.

// ─────────────────────────────────────────────────────────────────────────────
// Arguments étendus optionnels
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments étendus de lecture passés optionnellement via `args_ptr`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ReadArgs {
    /// Offset de lecture dans le blob. Si `use_cursor != 0`, ignoré.
    pub offset:     u64,
    /// Nombre d'octets maximal à lire.
    pub count:      u64,
    /// Si non-zéro, utilise et avance le curseur du fd.
    pub use_cursor: u32,
    /// Flags optionnels (0 = défaut).
    pub flags:      u32,
}

const _: () = assert!(core::mem::size_of::<ReadArgs>() == 24);

impl ReadArgs {
    fn defaults(offset: u64, count: u64) -> Self {
        Self { offset, count, use_cursor: 0, flags: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de lecture
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération de lecture.
pub struct ReadResult {
    /// Nombre d'octets effectivement lus.
    pub bytes_read: usize,
    /// Nouvel offset après la lecture.
    pub new_offset: u64,
    /// True si l'EOF a été atteint.
    pub eof:        bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique de lecture
// ─────────────────────────────────────────────────────────────────────────────

/// Lit `count` octets depuis le blob `blob_id` à partir de `offset`.
///
/// Remplit `buf` et retourne le nombre d'octets lus.
/// Si `offset >= blob_size`, retourne Ok(0) (EOF).
///
/// OOM-02 : le buffer est fourni par l'appelant (alloué via try_reserve).
fn read_blob(
    blob_id: BlobId,
    offset:  u64,
    count:   usize,
    buf:     &mut [u8],
) -> ExofsResult<ReadResult> {
    // Récupérer le blob depuis le cache.
    let data = BLOB_CACHE.get(&blob_id).ok_or(ExofsError::BlobNotFound)?;
    let blob_size = data.len();

    if offset >= blob_size as u64 {
        return Ok(ReadResult { bytes_read: 0, new_offset: offset, eof: true });
    }

    let start = offset as usize;
    // ARITH-02 : checked_add pour éviter overflow.
    let end = start.checked_add(count)
        .map(|e| e.min(blob_size))
        .unwrap_or(blob_size);
    let n = end.saturating_sub(start);

    // Copier dans buf (RECUR-01 : while).
    let mut i = 0usize;
    while i < n {
        buf[i] = data[start.wrapping_add(i)];
        i = i.wrapping_add(1);
    }

    let new_offset = start.wrapping_add(n) as u64;
    Ok(ReadResult {
        bytes_read: n,
        new_offset,
        eof: new_offset >= blob_size as u64,
    })
}

/// Effectue une lecture complète depuis un fd.
///
/// Si `use_cursor`, l'offset courant du fd est utilisé et le curseur avancé.
fn read_fd(
    fd:         u32,
    offset:     u64,
    count:      usize,
    use_cursor: bool,
    buf:        &mut Vec<u8>,
) -> ExofsResult<ReadResult> {
    // Vérification des droits.
    OBJECT_TABLE.check_readable(fd)?;

    // Récupérer le blob_id.
    let entry = OBJECT_TABLE.get(fd)?;
    let blob_id = entry.blob_id;

    // Déterminer l'offset effectif.
    let effective_offset = if use_cursor { entry.cursor } else { offset };

    // S'assurer que le buffer est suffisant (OOM-02).
    if buf.len() < count {
        buf.try_reserve(count.saturating_sub(buf.len()))
            .map_err(|_| ExofsError::NoMemory)?;
        buf.resize(count, 0u8);
    }

    // Lire le blob.
    let result = read_blob(blob_id, effective_offset, count, &mut buf[..count])?;

    // Avancer le curseur si demandé.
    if use_cursor && result.bytes_read > 0 {
        OBJECT_TABLE.set_cursor(fd, result.new_offset)?;
    }

    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall SYS_EXOFS_OBJECT_READ (502)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_read(fd, buf_ptr, count, offset, args_ptr, _) → bytes_read ou errno`
///
/// - `fd`       : descripteur de fichier ExoFS (≥ 4).
/// - `buf_ptr`  : pointeur userspace vers le buffer de destination.
/// - `count`    : nombre d'octets à lire.
/// - `offset`   : offset dans le blob (ignoré si ReadArgs.use_cursor != 0).
/// - `args_ptr` : pointeur optionnel vers `ReadArgs`.
pub fn sys_exofs_object_read(
    fd:       u64,
    buf_ptr:  u64,
    count:    u64,
    offset:   u64,
    args_ptr: u64,
    _a6:      u64,
) -> i64 {
    // 1. Valider les arguments de base.
    let fd_u32 = match validate_fd(fd) {
        Ok(f)  => f,
        Err(e) => return e,
    };
    if buf_ptr == 0 { return EFAULT; }
    let count_usize = match validate_count(count) {
        Ok(c)  => c.min(READ_MAX_BYTES),
        Err(e) => return e,
    };
    let offset_val = match validate_offset(offset) {
        Ok(o)  => o,
        Err(e) => return e,
    };

    // 2. Lire les ReadArgs optionnels.
    let (effective_offset, effective_count, use_cursor) = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<ReadArgs>(args_ptr) } {
            Ok(a) => {
                let off = if a.use_cursor != 0 { offset_val } else { a.offset };
                let cnt = (a.count as usize).min(READ_MAX_BYTES);
                (off, cnt, a.use_cursor != 0)
            }
            Err(_) => return EFAULT,
        }
    } else {
        (offset_val, count_usize, false)
    };

    if effective_count == 0 { return 0; }

    // 3. Allouer le buffer de lecture sur le tas (RÈGLE 10, OOM-02).
    let mut read_buf: Vec<u8> = Vec::new();
    read_buf.try_reserve(effective_count).map_err(|_| ENOMEM as i64)
        .and_then(|_| {
            read_buf.resize(effective_count, 0u8);
            // 4. Lire depuis le fd.
            read_fd(fd_u32, effective_offset, effective_count, use_cursor, &mut read_buf)
                .map_err(|e| exofs_err_to_errno(e))
                .and_then(|result| {
                    if result.bytes_read == 0 {
                        return Ok(0i64); // EOF
                    }
                    // 5. Copier vers userspace (RÈGLE 9 : copy_to_user).
                    write_user_buf(buf_ptr, &read_buf[..result.bytes_read])?;
                    Ok(result.bytes_read as i64)
                })
        })
        .unwrap_or_else(|e| e)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le nombre d'octets restants dans un blob à partir d'un offset.
/// ARITH-02 : saturating_sub.
#[inline]
pub fn bytes_remaining(blob_size: u64, offset: u64) -> u64 {
    blob_size.saturating_sub(offset)
}

/// Calcule le nombre de pages complètes contenues dans `bytes`.
/// ARITH-02 : checked_div.
#[inline]
pub fn pages_in(bytes: u64, page_size: u64) -> u64 {
    bytes.checked_div(page_size.max(1)).unwrap_or(0)
}

/// Aligne un offset vers le haut sur `align` (puissance de 2).
/// ARITH-02 : checked_add + wrapping_sub + masque.
#[inline]
pub fn align_up(offset: u64, align: u64) -> Option<u64> {
    if align == 0 { return None; }
    let mask = align.wrapping_sub(1);
    offset.checked_add(mask).map(|v| v & !mask)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_remaining_normal() {
        assert_eq!(bytes_remaining(1024, 100), 924);
    }

    #[test]
    fn test_bytes_remaining_past_end() {
        assert_eq!(bytes_remaining(100, 200), 0);
    }

    #[test]
    fn test_bytes_remaining_zero_offset() {
        assert_eq!(bytes_remaining(4096, 0), 4096);
    }

    #[test]
    fn test_pages_in() {
        assert_eq!(pages_in(4096, 4096), 1);
        assert_eq!(pages_in(8192, 4096), 2);
        assert_eq!(pages_in(100, 4096), 0);
    }

    #[test]
    fn test_pages_in_zero_page_size() {
        assert_eq!(pages_in(4096, 0), 0); // checked_div evite division par zéro.
    }

    #[test]
    fn test_align_up_4k() {
        assert_eq!(align_up(0, 4096), Some(0));
        assert_eq!(align_up(1, 4096), Some(4096));
        assert_eq!(align_up(4096, 4096), Some(4096));
        assert_eq!(align_up(4097, 4096), Some(8192));
    }

    #[test]
    fn test_align_up_zero_align() {
        assert_eq!(align_up(100, 0), None);
    }

    #[test]
    fn test_read_args_size() {
        assert_eq!(core::mem::size_of::<ReadArgs>(), 24);
    }

    #[test]
    fn test_read_args_defaults() {
        let a = ReadArgs::defaults(512, 1024);
        assert_eq!(a.offset, 512);
        assert_eq!(a.count, 1024);
        assert_eq!(a.use_cursor, 0);
    }

    #[test]
    fn test_sys_read_null_buf() {
        // buf_ptr == 0 → EFAULT
        assert_eq!(sys_exofs_object_read(4, 0, 100, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_sys_read_bad_fd() {
        assert_eq!(sys_exofs_object_read(0, 0x1000, 100, 0, 0, 0), EBADF);
    }

    #[test]
    fn test_sys_read_zero_count() {
        assert_eq!(sys_exofs_object_read(4, 0x1000, 0, 0, 0, 0), EINVAL);
    }

    #[test]
    fn test_sys_read_count_too_large() {
        let big: u64 = (super::super::validation::EXOFS_BLOB_MAX as u64) + 1;
        assert_eq!(sys_exofs_object_read(4, 0x1000, big, 0, 0, 0), ERANGE);
    }

    #[test]
    fn test_read_blob_eof() {
        // Créer un BlobId inexistant dans le cache → BlobNotFound.
        let blob = BlobId([0xABu8; 32]);
        let mut buf = [0u8; 64];
        let r = read_blob(blob, 0, 64, &mut buf);
        // BlobNotFound car pas dans le cache (non fatal, comportement attendu).
        assert!(r.is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scatter-read : lecture en plusieurs segments
// ─────────────────────────────────────────────────────────────────────────────

/// Un segment de scatter-read : offset + longueur dans le blob.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadSegment {
    /// Offset dans le blob.
    pub offset: u64,
    /// Nombre d'octets à lire.
    pub count:  u64,
    /// Pointeur userspace vers le buffer de destination.
    pub buf_ptr:u64,
}

const _: () = assert!(core::mem::size_of::<ReadSegment>() == 24);

/// Nombre maximum de segments de scatter-read dans un seul appel.
pub const MAX_SCATTER_SEGMENTS: usize = 16;

/// Effectue un scatter-read sur un seul BlobId : plusieurs segments disjoints.
///
/// Retourne le nombre total d'octets lus.
/// RECUR-01 : while.
/// OOM-02 : try_reserve pour le buffer temporaire.
pub fn scatter_read(
    blob_id:  BlobId,
    segments: &[ReadSegment],
) -> ExofsResult<u64> {
    let n_segs = segments.len().min(MAX_SCATTER_SEGMENTS);
    let mut total: u64 = 0u64;
    let mut seg_idx = 0usize;

    while seg_idx < n_segs {
        let seg = &segments[seg_idx];
        if seg.count == 0 || seg.buf_ptr == 0 {
            seg_idx = seg_idx.wrapping_add(1);
            continue;
        }
        let count = (seg.count as usize).min(READ_MAX_BYTES);

        // Buffer temporaire sur le tas (OOM-02).
        let mut tmp: Vec<u8> = Vec::new();
        tmp.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
        tmp.resize(count, 0u8);

        let result = read_blob(blob_id, seg.offset, count, &mut tmp)?;
        if result.bytes_read > 0 {
            // Copier vers userspace (RÈGLE 9).
            // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
            unsafe {
                super::validation::copy_to_user(
                    seg.buf_ptr as *mut u8,
                    tmp.as_ptr(),
                    result.bytes_read,
                ).map_err(|_| ExofsError::IoError)?;
            }
            total = total.saturating_add(result.bytes_read as u64);
        }
        seg_idx = seg_idx.wrapping_add(1);
    }
    Ok(total)
}

/// Calcule combien d'octets peuvent être lus sans dépasser la fin du blob.
/// ARITH-02 : checked_add, saturating_sub.
#[inline]
pub fn clamp_read_count(offset: u64, count: usize, blob_size: u64) -> usize {
    if offset >= blob_size { return 0; }
    let remaining = blob_size.saturating_sub(offset);
    count.min(remaining as usize)
}

/// Vérifie si un offset+count est valide pour un blob de taille donnée.
/// ARITH-02 : checked_add.
#[inline]
pub fn is_range_valid(offset: u64, count: u64, blob_size: u64) -> bool {
    if count == 0 { return true; }
    offset.checked_add(count).map(|end| end <= blob_size).unwrap_or(false)
}

#[cfg(test)]
mod tests_extended {
    use super::*;

    #[test]
    fn test_clamp_read_past_end() {
        assert_eq!(clamp_read_count(900, 200, 1000), 100);
    }

    #[test]
    fn test_clamp_read_at_end() {
        assert_eq!(clamp_read_count(1000, 100, 1000), 0);
    }

    #[test]
    fn test_clamp_read_within() {
        assert_eq!(clamp_read_count(0, 100, 4096), 100);
    }

    #[test]
    fn test_is_range_valid_ok() {
        assert!(is_range_valid(0, 100, 4096));
        assert!(is_range_valid(4090, 6, 4096));
    }

    #[test]
    fn test_is_range_valid_overflow() {
        assert!(!is_range_valid(u64::MAX - 10, 20, u64::MAX));
    }

    #[test]
    fn test_is_range_valid_past_end() {
        assert!(!is_range_valid(4090, 10, 4096));
    }

    #[test]
    fn test_is_range_valid_zero_count() {
        assert!(is_range_valid(9999, 0, 100)); // Zero count toujours valide.
    }

    #[test]
    fn test_read_segment_size() {
        assert_eq!(core::mem::size_of::<ReadSegment>(), 24);
    }

    #[test]
    fn test_scatter_read_empty_segments() {
        let blob = BlobId([0xCCu8; 32]);
        let segs: &[ReadSegment] = &[];
        let r = scatter_read(blob, segs).unwrap();
        assert_eq!(r, 0);
    }
}
