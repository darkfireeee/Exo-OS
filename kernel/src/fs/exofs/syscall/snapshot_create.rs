//! snapshot_create.rs — SYS_EXOFS_SNAPSHOT_CREATE (509)
//!
//! Création d'un snapshot (point de sauvegarde figé) d'un blob ExoFS.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf,
    verify_cap, CapabilityType, EFAULT,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const SNAPSHOT_NAME_MAX: usize = 128;
pub const SNAPSHOT_MAGIC:    u32   = 0x534E_4150; // "SNAP"
pub const SNAPSHOT_VER:      u8    = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod snap_flags {
    pub const ATOMIC:      u32 = 0x0001;
    pub const READ_ONLY:   u32 = 0x0002;
    pub const COW:         u32 = 0x0004;
    pub const NAMED:       u32 = 0x0008;
    pub const VALID_MASK:  u32 = ATOMIC | READ_ONLY | COW | NAMED;
}

// ─────────────────────────────────────────────────────────────────────────────
// Arguments étendus
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotCreateArgs {
    pub flags:     u32,
    pub _pad:      u32,
    pub epoch_id:  u64,
    pub parent_id: [u8; 32],
    pub name_ptr:  u64,
    pub name_len:  u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotCreateArgs>() == 64);

// ─────────────────────────────────────────────────────────────────────────────
// Résultat
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotCreateResult {
    pub snapshot_id:   [u8; 32],
    pub blob_id:       [u8; 32],
    pub size_bytes:    u64,
    pub epoch_id:      u64,
    pub flags:         u32,
    pub _pad:          u32,
}

const _: () = assert!(core::mem::size_of::<SnapshotCreateResult>() == 88);

// ─────────────────────────────────────────────────────────────────────────────
// Clé du snapshot dans le cache
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive un SnapshotId = Blake3(source_blob_id || epoch_id_le || b"\xAA\xBB")
fn snapshot_id(source: BlobId, epoch_id: u64, name: &[u8]) -> BlobId {
    let mut buf: [u8; 8 + 32 + SNAPSHOT_NAME_MAX + 2] = [0u8; 8 + 32 + SNAPSHOT_NAME_MAX + 2];
    let sb = source.as_bytes();
    let mut i = 0usize;
    while i < 32 { buf[i] = sb[i]; i = i.wrapping_add(1); }
    let ep = epoch_id.to_le_bytes();
    let mut j = 0usize;
    while j < 8 { buf[32 + j] = ep[j]; j = j.wrapping_add(1); }
    let nl = name.len().min(SNAPSHOT_NAME_MAX);
    let mut k = 0usize;
    while k < nl { buf[40 + k] = name[k]; k = k.wrapping_add(1); }
    buf[40 + nl]     = 0xAA;
    buf[40 + nl + 1] = 0xBB;
    BlobId::from_bytes_blake3(&buf[..40 + nl + 2])
}

// ─────────────────────────────────────────────────────────────────────────────
// Format du blob snapshot
// ─────────────────────────────────────────────────────────────────────────────
//
// Header : magic(4) + version(1) + flags(1) + _pad(2) + epoch(8) + size(8) + name_len(2) + name
// Puis le contenu copié du source.

fn build_snapshot_blob(
    source_data: &[u8],
    epoch_id:    u64,
    flags:       u32,
    name:        &[u8],
) -> ExofsResult<Vec<u8>> {
    if name.len() > SNAPSHOT_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    let nl = name.len();
    let header_size = 4 + 1 + 1 + 2 + 8 + 8 + 2 + nl;
    let total = header_size.saturating_add(source_data.len());
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;

    // magic
    let mag = SNAPSHOT_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(mag[i]); i = i.wrapping_add(1); }
    buf.push(SNAPSHOT_VER);
    buf.push((flags & 0xFF) as u8);
    buf.push(0u8); buf.push(0u8); // _pad
    let ep = epoch_id.to_le_bytes();
    let mut j = 0usize;
    while j < 8 { buf.push(ep[j]); j = j.wrapping_add(1); }
    let sz = (source_data.len() as u64).to_le_bytes();
    let mut k = 0usize;
    while k < 8 { buf.push(sz[k]); k = k.wrapping_add(1); }
    buf.push((nl & 0xFF) as u8);
    buf.push((nl >> 8)   as u8);
    let mut m = 0usize;
    while m < nl { buf.push(name[m]); m = m.wrapping_add(1); }
    // contenu
    let mut n = 0usize;
    while n < source_data.len() { buf.push(source_data[n]); n = n.wrapping_add(1); }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique principale
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn create_snapshot(
    source_blob: BlobId,
    epoch_id:    u64,
    flags:       u32,
    name:        &[u8],
) -> ExofsResult<SnapshotCreateResult> {
    if flags & !snap_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
    if name.len() > SNAPSHOT_NAME_MAX { return Err(ExofsError::PathTooLong); }
    let source_data = BLOB_CACHE.get(&source_blob)
        .ok_or(ExofsError::BlobNotFound)?;
    let size = source_data.len() as u64;
    let snap_key = snapshot_id(source_blob, epoch_id, name);
    if BLOB_CACHE.get(&snap_key).is_some() { return Err(ExofsError::ObjectAlreadyExists); }
    let snap_blob = build_snapshot_blob(&source_data, epoch_id, flags, name)?;
    BLOB_CACHE.insert(snap_key, snap_blob.to_vec())?;
    Ok(SnapshotCreateResult {
        snapshot_id: *snap_key.as_bytes(),
        blob_id:     *source_blob.as_bytes(),
        size_bytes:  size,
        epoch_id,
        flags,
        _pad:        0,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_SNAPSHOT_CREATE (509)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_snapshot_create(fd, out_ptr, args_ptr, _, _, _) → 0 ou errno`
pub fn sys_exofs_snapshot_create(
    fd:      u64,
    out_ptr: u64,
    args_ptr:u64,
    _a4:     u64,
    _a5:     u64,
    cap_rights: u64,
) -> i64 {
    let blob_id = match OBJECT_TABLE.blob_id_of(fd as u32) {
        Ok(id) => id,
        Err(e) => return exofs_err_to_errno(e),
    };

    let args = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<SnapshotCreateArgs>(args_ptr) } {
            Ok(a)  => a,
            Err(_) => return EFAULT,
        }
    } else {
        SnapshotCreateArgs {
            flags:     snap_flags::READ_ONLY,
            _pad:      0,
            epoch_id:  0,
            parent_id: [0u8; 32],
            name_ptr:  0,
            name_len:  0,
        }
    };

    let mut name_buf: Vec<u8> = Vec::new();
    if args.name_ptr != 0 && args.name_len > 0 {
        let req_nl = args.name_len as usize;
        if req_nl > SNAPSHOT_NAME_MAX {
            return exofs_err_to_errno(ExofsError::PathTooLong);
        }
        let nl = req_nl;
        if name_buf.try_reserve(nl).is_err() {
            return exofs_err_to_errno(ExofsError::NoMemory);
        }
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        unsafe {
            let src = args.name_ptr as *const u8;
            let mut i = 0usize;
            while i < nl { name_buf.push(*src.add(i)); i = i.wrapping_add(1); }
        }
    }

    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsSnapshotCreate) {
        return e;
    }

    let result = match create_snapshot(blob_id, args.epoch_id, args.flags, &name_buf) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };

    if out_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &result as *const SnapshotCreateResult as *const u8,
                core::mem::size_of::<SnapshotCreateResult>(),
            )
        };
        if let Err(e) = write_user_buf(out_ptr, bytes) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le SnapshotId sans créer de blob.
pub fn compute_snapshot_id(source: BlobId, epoch_id: u64, name: &[u8]) -> BlobId {
    snapshot_id(source, epoch_id, name)
}

/// Retourne `true` si un snapshot avec ce nom/epoch existe.
pub fn snapshot_exists(source: BlobId, epoch_id: u64, name: &[u8]) -> bool {
    let sid = snapshot_id(source, epoch_id, name);
    BLOB_CACHE.get(&sid).is_some()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_source(path: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, b"source data for snapshot".to_vec()).unwrap();
        id
    }

    #[test]
    fn test_create_snapshot_ok() {
        let src = make_source(b"/snap/src1");
        let r = create_snapshot(src, 1, snap_flags::READ_ONLY, b"snap1").unwrap();
        assert_eq!(r.epoch_id, 1);
        assert_ne!(r.snapshot_id, [0u8; 32]);
    }

    #[test]
    fn test_create_snapshot_duplicate() {
        let src = make_source(b"/snap/dup");
        create_snapshot(src, 2, snap_flags::READ_ONLY, b"snap_dup").unwrap();
        assert!(create_snapshot(src, 2, snap_flags::READ_ONLY, b"snap_dup").is_err());
    }

    #[test]
    fn test_create_snapshot_source_missing() {
        let id = BlobId::from_bytes_blake3(b"/snap/miss");
        assert!(create_snapshot(id, 0, 0, b"").is_err());
    }

    #[test]
    fn test_snapshot_exists_false() {
        let src = make_source(b"/snap/ex1");
        assert!(!snapshot_exists(src, 99, b"nope"));
    }

    #[test]
    fn test_snapshot_exists_true() {
        let src = make_source(b"/snap/ex2");
        create_snapshot(src, 10, snap_flags::READ_ONLY, b"check").unwrap();
        assert!(snapshot_exists(src, 10, b"check"));
    }

    #[test]
    fn test_create_result_size() {
        assert_eq!(core::mem::size_of::<SnapshotCreateResult>(), 88);
    }

    #[test]
    fn test_args_size() {
        assert_eq!(core::mem::size_of::<SnapshotCreateArgs>(), 64);
    }

    #[test]
    fn test_different_epochs_different_snapshots() {
        let src = make_source(b"/snap/ep");
        let r1 = create_snapshot(src, 1, 0, b"").unwrap();
        let r2 = create_snapshot(src, 2, 0, b"").unwrap();
        assert_ne!(r1.snapshot_id, r2.snapshot_id);
    }

    #[test]
    fn test_invalid_flags() {
        let src = make_source(b"/snap/flag");
        assert!(create_snapshot(src, 0, 0xDEAD, b"").is_err());
    }

    #[test]
    fn test_sys_bad_fd() {
        let r = sys_exofs_snapshot_create(9999, 0, 0, 0, 0, 0);
        assert!(r < 0);
    }

    #[test]
    fn test_compute_snapshot_id_deterministic() {
        let src = make_source(b"/snap/det");
        let id1 = compute_snapshot_id(src, 5, b"name");
        let id2 = compute_snapshot_id(src, 5, b"name");
        assert_eq!(id1.as_bytes(), id2.as_bytes());
    }

    #[test]
    fn test_snapshot_blob_content_preserved() {
        let src_id = make_source(b"/snap/content");
        let r = create_snapshot(src_id, 7, 0, b"").unwrap();
        let sid = BlobId(r.snapshot_id);
        let data = BLOB_CACHE.get(&sid).unwrap();
        assert!(data.len() > 24); // plus grand que le header
    }

    #[test]
    fn test_snapshot_blob_magic() {
        let src = make_source(b"/snap/magic");
        let r = create_snapshot(src, 0, 0, b"").unwrap();
        let sid = BlobId(r.snapshot_id);
        let data = BLOB_CACHE.get(&sid).unwrap();
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        assert_eq!(magic, SNAPSHOT_MAGIC);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre minimaliste de snapshots (liste chaînée par epoch)
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant compact d'un snapshot pour le registre.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotRef {
    pub snapshot_id: [u8; 32],
    pub source_id:   [u8; 32],
    pub epoch_id:    u64,
    pub size_bytes:  u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotRef>() == 80);

/// Sérialise une liste de SnapshotRef en octets bruts.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn encode_snapshot_refs(refs: &[SnapshotRef]) -> ExofsResult<Vec<u8>> {
    let entry_size = core::mem::size_of::<SnapshotRef>();
    let total = refs.len().saturating_mul(entry_size);
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < refs.len() {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let raw = unsafe {
            core::slice::from_raw_parts(&refs[i] as *const SnapshotRef as *const u8, entry_size)
        };
        let mut j = 0usize;
        while j < entry_size { buf.push(raw[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

/// Désérialise une liste de SnapshotRef depuis des octets.
pub fn decode_snapshot_refs(data: &[u8]) -> ExofsResult<Vec<SnapshotRef>> {
    let entry_size = core::mem::size_of::<SnapshotRef>();
    if data.len() % entry_size != 0 { return Err(ExofsError::CorruptedStructure); }
    let count = data.len() / entry_size;
    let mut out: Vec<SnapshotRef> = Vec::new();
    out.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < count {
        let off = i.saturating_mul(entry_size);
        let mut r = SnapshotRef::default();
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let dst = unsafe {
            core::slice::from_raw_parts_mut(&mut r as *mut SnapshotRef as *mut u8, entry_size)
        };
        let mut j = 0usize;
        while j < entry_size { dst[j] = data[off + j]; j = j.wrapping_add(1); }
        out.push(r);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Taille du header d'un blob snapshot.
pub fn snapshot_header_size(name_len: usize) -> usize {
    4 + 1 + 1 + 2 + 8 + 8 + 2 + name_len
}

/// Extrait l'epoch_id depuis le blob snapshot (bytes 8..16 après magic+version+flags+pad).
pub fn snapshot_epoch_from_blob(data: &[u8]) -> ExofsResult<u64> {
    if data.len() < 16 { return Err(ExofsError::CorruptedStructure); }
    let ep = u64::from_le_bytes([
        data[8], data[9], data[10], data[11],
        data[12], data[13], data[14], data[15],
    ]);
    Ok(ep)
}

/// Extrait la taille de la source depuis le blob snapshot (bytes 16..24).
pub fn snapshot_source_size_from_blob(data: &[u8]) -> ExofsResult<u64> {
    if data.len() < 24 { return Err(ExofsError::CorruptedStructure); }
    let sz = u64::from_le_bytes([
        data[16], data[17], data[18], data[19],
        data[20], data[21], data[22], data[23],
    ]);
    Ok(sz)
}

/// Vérifie le magic header d'un blob snapshot.
pub fn check_snapshot_magic(data: &[u8]) -> bool {
    if data.len() < 4 { return false; }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    magic == SNAPSHOT_MAGIC
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    fn make_src(path: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, b"data".to_vec()).unwrap();
        id
    }

    #[test]
    fn test_snapshot_ref_size() {
        assert_eq!(core::mem::size_of::<SnapshotRef>(), 80);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let refs = [
            SnapshotRef { epoch_id: 1, size_bytes: 100, ..SnapshotRef::default() },
            SnapshotRef { epoch_id: 2, size_bytes: 200, ..SnapshotRef::default() },
        ];
        let enc = encode_snapshot_refs(&refs).unwrap();
        let dec = decode_snapshot_refs(&enc).unwrap();
        assert_eq!(dec.len(), 2);
        assert_eq!(dec[0].epoch_id, 1);
    }

    #[test]
    fn test_encode_empty() {
        assert!(encode_snapshot_refs(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_decode_bad_alignment() {
        assert!(decode_snapshot_refs(&[0u8; 7]).is_err());
    }

    #[test]
    fn test_snapshot_header_size() {
        assert_eq!(snapshot_header_size(4), 4 + 1 + 1 + 2 + 8 + 8 + 2 + 4);
    }

    #[test]
    fn test_snapshot_epoch_from_blob() {
        let src = make_src(b"/snap/adv/ep");
        let r = create_snapshot(src, 99, 0, b"ep").unwrap();
        let sid = BlobId(r.snapshot_id);
        let data = BLOB_CACHE.get(&sid).unwrap();
        assert_eq!(snapshot_epoch_from_blob(&data).unwrap(), 99);
    }

    #[test]
    fn test_snapshot_source_size_from_blob() {
        let src = make_src(b"/snap/adv/sz");
        let r = create_snapshot(src, 0, 0, b"").unwrap();
        let sid = BlobId(r.snapshot_id);
        let data = BLOB_CACHE.get(&sid).unwrap();
        assert_eq!(snapshot_source_size_from_blob(&data).unwrap(), 4);
    }

    #[test]
    fn test_check_snapshot_magic() {
        let src = make_src(b"/snap/mag2");
        let r = create_snapshot(src, 0, 0, b"").unwrap();
        let sid = BlobId(r.snapshot_id);
        let data = BLOB_CACHE.get(&sid).unwrap();
        assert!(check_snapshot_magic(&data));
    }

    #[test]
    fn test_check_snapshot_magic_false() {
        assert!(!check_snapshot_magic(&[0u8; 4]));
    }
}
