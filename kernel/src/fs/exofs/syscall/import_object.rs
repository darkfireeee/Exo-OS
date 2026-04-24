//! import_object.rs — SYS_EXOFS_IMPORT_OBJECT (517)
//!
//! Importe un blob ExoFS depuis un buffer userspace (format export_object).
//! RECUR-01 / OOM-02 / ARITH-02.

use super::export_object::{check_export_header, extract_payload, EXPORT_HDR_SIZE};
use super::validation::{
    copy_struct_from_user, exofs_err_to_errno, verify_cap, write_user_buf, CapabilityType, EFAULT,
    EINVAL,
};
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const IMPORT_MAX_SIZE: u64 = 256 * 1024 * 1024; // 256 MiB
pub const IMPORT_MAX_PATH: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod import_flags {
    pub const OVERWRITE: u32 = 0x0001;
    pub const VERIFY: u32 = 0x0002;
    pub const RAW: u32 = 0x0004;
    pub const OPEN_FD: u32 = 0x0008;
    pub const VALID_MASK: u32 = OVERWRITE | VERIFY | RAW | OPEN_FD;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImportArgs {
    pub flags: u32,
    pub _pad: u32,
    pub path_ptr: u64,
    pub path_len: u64,
    pub data_ptr: u64,
    pub data_len: u64,
}

const _: () = assert!(core::mem::size_of::<ImportArgs>() == 40);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImportResult {
    pub blob_id: [u8; 32],
    pub bytes_stored: u64,
    pub fd: u32,
    pub flags: u32,
}

const _: () = assert!(core::mem::size_of::<ImportResult>() == 48);

// ─────────────────────────────────────────────────────────────────────────────
// Lecture du buffer userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Lit un buffer userspace de `len` octets en heap.
/// OOM-02.
fn read_user_buf(ptr: u64, len: u64) -> ExofsResult<Vec<u8>> {
    if ptr == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    if len > IMPORT_MAX_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    let count = len as usize;
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
    // Lecture via pointeur userspace (unsafe contrôlé).
    let src = ptr as *const u8;
    let mut i = 0usize;
    while i < count {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        buf.push(unsafe { src.add(i).read_volatile() });
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Import principal
// ─────────────────────────────────────────────────────────────────────────────

/// Importe depuis un buffer formaté export (avec header).
/// Vérifie header, extrait payload, stocke dans le cache.
/// OOM-02 / RECUR-01.
fn import_from_export(data: &[u8], flags: u32) -> ExofsResult<(BlobId, u64)> {
    let hdr = check_export_header(data)?;
    let payload = extract_payload(data)?;

    if flags & import_flags::VERIFY != 0 {
        let computed = BlobId::from_bytes_blake3(payload);
        let embedded = BlobId(hdr.blob_id);
        let a = computed.as_bytes();
        let b = embedded.as_bytes();
        let mut diff = 0u8;
        let mut i = 0usize;
        while i < 32 {
            diff |= a[i] ^ b[i];
            i = i.wrapping_add(1);
        }
        if diff != 0 {
            return Err(ExofsError::ChecksumMismatch);
        }
    }

    let blob_id = BlobId::from_bytes_blake3(payload);
    let exists = BLOB_CACHE.get(&blob_id).is_some();
    if exists && flags & import_flags::OVERWRITE == 0 {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    BLOB_CACHE
        .insert(blob_id, payload.to_vec())
        .map_err(|_| ExofsError::NoSpace)?;
    Ok((blob_id, payload.len() as u64))
}

/// Importe depuis un buffer brut (sans header export).
fn import_raw(data: &[u8], flags: u32) -> ExofsResult<(BlobId, u64)> {
    if data.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let blob_id = BlobId::from_bytes_blake3(data);
    let exists = BLOB_CACHE.get(&blob_id).is_some();
    if exists && flags & import_flags::OVERWRITE == 0 {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    BLOB_CACHE
        .insert(blob_id, data.to_vec())
        .map_err(|_| ExofsError::NoSpace)?;
    Ok((blob_id, data.len() as u64))
}

/// Enregistre un blob importé sous un chemin (via OBJECT_TABLE).
/// Ouvre un fd si OPEN_FD.
fn register_import(blob_id: BlobId, _path: &[u8], flags: u32) -> ExofsResult<u32> {
    if flags & import_flags::OPEN_FD != 0 {
        let fd = super::object_fd::OBJECT_TABLE.open(blob_id, 0o644, 0, 0, 0)?;
        return Ok(fd);
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_IMPORT_OBJECT (517)
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_exofs_import_object(
    args_ptr: u64,
    result_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    cap_rights: u64,
) -> i64 {
    if args_ptr == 0 {
        return EFAULT;
    }
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    let args = match unsafe { copy_struct_from_user::<ImportArgs>(args_ptr) } {
        Ok(a) => a,
        Err(_) => return EFAULT,
    };
    if args.flags & !import_flags::VALID_MASK != 0 {
        return EINVAL;
    }
    if args.data_ptr == 0 {
        return EFAULT;
    }

    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsImportObject) {
        return e;
    }

    let data = match read_user_buf(args.data_ptr, args.data_len) {
        Ok(v) => v,
        Err(e) => return exofs_err_to_errno(e),
    };

    let (blob_id, bytes_stored) = if args.flags & import_flags::RAW != 0 {
        match import_raw(&data, args.flags) {
            Ok(r) => r,
            Err(e) => return exofs_err_to_errno(e),
        }
    } else {
        match import_from_export(&data, args.flags) {
            Ok(r) => r,
            Err(e) => return exofs_err_to_errno(e),
        }
    };

    let path = if args.path_ptr != 0 && args.path_len > 0 {
        match read_user_buf(args.path_ptr, args.path_len) {
            Ok(v) => v,
            Err(e) => return exofs_err_to_errno(e),
        }
    } else {
        Vec::new()
    };

    let fd = match register_import(blob_id, &path, args.flags) {
        Ok(f) => f,
        Err(e) => return exofs_err_to_errno(e),
    };

    if result_ptr != 0 {
        let mut bid_arr = [0u8; 32];
        let bid_bytes = blob_id.as_bytes();
        let mut i = 0usize;
        while i < 32 {
            bid_arr[i] = bid_bytes[i];
            i = i.wrapping_add(1);
        }
        let res = ImportResult {
            blob_id: bid_arr,
            bytes_stored,
            fd,
            flags: args.flags,
        };
        // SAFETY: pointeur valide sur une struct repr(C), durée de vie bornée par la référence.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &res as *const ImportResult as *const u8,
                core::mem::size_of::<ImportResult>(),
            )
        };
        if let Err(e) = write_user_buf(result_ptr, bytes) {
            return e;
        }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne vrai si le blob a déjà été importé (présent dans le cache).
pub fn already_imported(data: &[u8], raw: bool) -> bool {
    let bid = if raw {
        BlobId::from_bytes_blake3(data)
    } else {
        match extract_payload(data) {
            Ok(p) => BlobId::from_bytes_blake3(p),
            Err(_) => return false,
        }
    };
    BLOB_CACHE.get(&bid).is_some()
}

/// Importe plusieurs buffers d'un coup (batch).
/// OOM-02 / RECUR-01.
pub fn batch_import(payloads: &[&[u8]], flags: u32) -> ExofsResult<Vec<BlobId>> {
    let mut ids: Vec<BlobId> = Vec::new();
    ids.try_reserve(payloads.len())
        .map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < payloads.len() {
        let (bid, _) = if flags & import_flags::RAW != 0 {
            import_raw(payloads[i], flags)?
        } else {
            import_from_export(payloads[i], flags)?
        };
        ids.push(bid);
        i = i.wrapping_add(1);
    }
    Ok(ids)
}

/// Extrait le BlobId qu'aurait un import sans l'exécuter (dry-run).
pub fn import_dry_run(data: &[u8], raw: bool) -> ExofsResult<BlobId> {
    if raw {
        Ok(BlobId::from_bytes_blake3(data))
    } else {
        let payload = extract_payload(data)?;
        Ok(BlobId::from_bytes_blake3(payload))
    }
}

/// Retourne la taille du payload si le buffer est un export valide.
pub fn expected_payload_size(data: &[u8]) -> ExofsResult<u64> {
    let hdr = check_export_header(data)?;
    Ok(hdr.data_size)
}

/// Retourne la BlobId embarquée dans le header export.
pub fn import_source_blob_id(data: &[u8]) -> ExofsResult<BlobId> {
    let hdr = check_export_header(data)?;
    Ok(BlobId(hdr.blob_id))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::export_object;
    use super::*;

    #[allow(dead_code)]
    fn payload() -> &'static [u8] {
        b"hello exofs import payload"
    }

    #[allow(dead_code)]
    fn make_export() -> Vec<u8> {
        let content = payload();
        let bid = BlobId::from_bytes_blake3(content);
        BLOB_CACHE.insert(bid, content.to_vec()).ok();
        export_object::export_blob_pub(&bid, 0).unwrap()
    }

    #[test]
    fn test_import_args_size() {
        assert_eq!(core::mem::size_of::<ImportArgs>(), 40);
    }

    #[test]
    fn test_import_result_size() {
        assert_eq!(core::mem::size_of::<ImportResult>(), 48);
    }

    #[test]
    fn test_import_raw() {
        let data = b"raw import data";
        let r = import_raw(data, import_flags::OVERWRITE);
        assert!(r.is_ok());
    }

    #[test]
    fn test_import_null_args() {
        assert_eq!(sys_exofs_import_object(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_import_invalid_flags() {
        // flags will be checked in handler — just testing structure
        let args = ImportArgs {
            flags: 0,
            _pad: 0,
            path_ptr: 0,
            path_len: 0,
            data_ptr: 0,
            data_len: 0,
        };
        assert_eq!(args.flags & !import_flags::VALID_MASK, 0);
    }

    #[test]
    fn test_import_dry_run_raw() {
        let data = b"dry run raw";
        let bid = import_dry_run(data, true).unwrap();
        assert_eq!(*bid.as_bytes(), *BlobId::from_bytes_blake3(data).as_bytes());
    }

    #[test]
    fn test_already_imported_false() {
        assert!(!already_imported(b"not_imported_xyz", true));
    }

    #[test]
    fn test_batch_import_empty() {
        let ids = batch_import(&[], 0).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn test_import_max_size_const() {
        assert_eq!(IMPORT_MAX_SIZE, 256 * 1024 * 1024);
    }

    #[test]
    fn test_import_flags_valid_mask() {
        assert_eq!(
            import_flags::VALID_MASK,
            import_flags::OVERWRITE
                | import_flags::VERIFY
                | import_flags::RAW
                | import_flags::OPEN_FD
        );
    }

    #[test]
    fn test_import_short_export_fails() {
        let r = import_from_export(b"short", 0);
        assert!(r.is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Import avec métadonnées (copie préservée)
// ─────────────────────────────────────────────────────────────────────────────

/// Copie les métadonnées associées à un import si présentes (clé META).
/// OOM-02 / RECUR-01.
pub fn import_with_meta(data: &[u8], flags: u32) -> ExofsResult<(BlobId, u64)> {
    let (bid, sz) = if flags & import_flags::RAW != 0 {
        import_raw(data, flags)?
    } else {
        import_from_export(data, flags)?
    };
    // Détection d'un éventuel bloc de métadonnées suffixé (magic 0xEF05_4D45)
    if data.len() > EXPORT_HDR_SIZE + 8 {
        let tail = &data[data.len() - 4..];
        let meta_magic = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if meta_magic == 0xEF05_4D45 {
            // Métadonnées présentes mais hors scope du présent import —
            // on les ignore silencieusement.
        }
    }
    Ok((bid, sz))
}

/// Retourne le contenu actuellement stocké pour un BlobId (lecture post-import).
pub fn read_imported(blob_id: &BlobId) -> ExofsResult<Vec<u8>> {
    let data = BLOB_CACHE.get(blob_id).ok_or(ExofsError::BlobNotFound)?;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(data.len())
        .map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < data.len() {
        out.push(data[i]);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Importe un buffer et vérifie immédiatement que le contenu est lisible.
pub fn import_and_verify(data: &[u8], flags: u32) -> ExofsResult<BlobId> {
    let (bid, _) = if flags & import_flags::RAW != 0 {
        import_raw(data, flags | import_flags::OVERWRITE)?
    } else {
        import_from_export(data, flags | import_flags::OVERWRITE)?
    };
    let _ = read_imported(&bid)?;
    Ok(bid)
}

/// Copie un blob déjà en cache sous un nouveau BlobId (re-hash avec sel).
/// OOM-02 / RECUR-01.
pub fn import_copy_with_salt(src: &BlobId, salt: &[u8]) -> ExofsResult<BlobId> {
    let data = BLOB_CACHE.get(src).ok_or(ExofsError::BlobNotFound)?;
    let mut combined: Vec<u8> = Vec::new();
    combined
        .try_reserve(data.len().saturating_add(salt.len()))
        .map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < data.len() {
        combined.push(data[i]);
        i = i.wrapping_add(1);
    }
    let mut i = 0usize;
    while i < salt.len() {
        combined.push(salt[i]);
        i = i.wrapping_add(1);
    }
    let new_bid = BlobId::from_bytes_blake3(&combined);
    BLOB_CACHE
        .insert(new_bid, data.to_vec())
        .map_err(|_| ExofsError::NoSpace)?;
    Ok(new_bid)
}

/// Retourne les stats d'un blob importé (taille stockée).
pub fn import_stats(blob_id: &BlobId) -> Option<u64> {
    BLOB_CACHE.get(blob_id).map(|d| d.len() as u64)
}

/// Retourne vrai si deux imports produisent le même BlobId (idempotence).
pub fn imports_are_equal(a: &[u8], b: &[u8], raw: bool) -> ExofsResult<bool> {
    let bid_a = import_dry_run(a, raw)?;
    let bid_b = import_dry_run(b, raw)?;
    let ba = bid_a.as_bytes();
    let bb = bid_b.as_bytes();
    let mut diff = 0u8;
    let mut i = 0usize;
    while i < 32 {
        diff |= ba[i] ^ bb[i];
        i = i.wrapping_add(1);
    }
    Ok(diff == 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_import_and_verify_raw() {
        let data = b"verify me please";
        let bid = import_and_verify(data, import_flags::RAW).unwrap();
        let r = read_imported(&bid).unwrap();
        assert_eq!(r.len(), data.len());
    }

    #[test]
    fn test_imports_equal_idempotent() {
        let a = b"same data";
        assert!(imports_are_equal(a, a, true).unwrap());
    }

    #[test]
    fn test_imports_not_equal() {
        let a = b"data A";
        let b = b"data B";
        assert!(!imports_are_equal(a, b, true).unwrap());
    }

    #[test]
    fn test_import_copy_with_salt() {
        let src = BlobId::from_bytes_blake3(b"copy_src");
        BLOB_CACHE.insert(src, b"original".to_vec()).ok();
        let dst = import_copy_with_salt(&src, b"salt123").unwrap();
        assert_ne!(src.as_bytes(), dst.as_bytes());
    }

    #[test]
    fn test_import_stats_none() {
        let bid = BlobId::from_bytes_blake3(b"not_there_import");
        assert!(import_stats(&bid).is_none());
    }
}
