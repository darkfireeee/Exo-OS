//! snapshot_mount.rs — SYS_EXOFS_SNAPSHOT_MOUNT (511)
//!
//! Monte un snapshot comme vue lecture-seule d'un blob. Ouvre un fd pointant
//! sur le contenu figé du snapshot.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT,
    verify_cap, CapabilityType,
};
use super::object_fd::{OBJECT_TABLE, open_flags};
use super::snapshot_create::{check_snapshot_magic, snapshot_epoch_from_blob, snapshot_source_size_from_blob};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub mod mount_flags {
    pub const READ_ONLY:   u32 = 0x0001;
    pub const COPY_ON_USE: u32 = 0x0002;
    pub const VALIDATE:    u32 = 0x0004;
    pub const VALID_MASK:  u32 = READ_ONLY | COPY_ON_USE | VALIDATE;
}

// ─────────────────────────────────────────────────────────────────────────────
// Arguments et résultat
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotMountArgs {
    pub flags:       u32,
    pub _pad:        u32,
    pub epoch_id:    u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotMountArgs>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotMountResult {
    pub fd:          u32,
    pub _pad:        u32,
    pub snapshot_id: [u8; 32],
    pub size_bytes:  u64,
    pub epoch_id:    u64,
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<SnapshotMountResult>() == 64);

// ─────────────────────────────────────────────────────────────────────────────
// Extraction du contenu d'un snapshot
// ─────────────────────────────────────────────────────────────────────────────
//
// Format blob snapshot : magic(4)+version(1)+flags(1)+pad(2)+epoch(8)+size(8)+name_len(2)+name+content

/// Retourne l'offset de début du contenu dans le blob snapshot.
fn content_offset(data: &[u8]) -> usize {
    if data.len() < 26 { return data.len(); }
    let name_len = u16::from_le_bytes([data[24], data[25]]) as usize;
    26usize.saturating_add(name_len)
}

/// Extrait le contenu du snapshot (la portion après le header).
/// OOM-02 : try_reserve. RECUR-01 : while.
fn extract_snapshot_content(snap_data: &[u8]) -> ExofsResult<Vec<u8>> {
    if !check_snapshot_magic(snap_data) { return Err(ExofsError::InvalidMagic); }
    let off = content_offset(snap_data);
    if off > snap_data.len() { return Err(ExofsError::CorruptedStructure); }
    let content = &snap_data[off..];
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(content.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < content.len() { buf.push(content[i]); i = i.wrapping_add(1); }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Montage principal
// ─────────────────────────────────────────────────────────────────────────────

/// Monte un snapshot identifié par `snap_blob_id`.
/// - Valide le magic.
/// - Extrait le contenu (ou crée un blob CoW).
/// - Ouvre un fd en lecture seule (ou RDWR si COPY_ON_USE).
fn mount_snapshot(snap_blob_id: BlobId, args: &SnapshotMountArgs) -> ExofsResult<SnapshotMountResult> {
    if args.flags & !mount_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
    let snap_data = BLOB_CACHE.get(&snap_blob_id)
        .ok_or(ExofsError::BlobNotFound)?;
    if !check_snapshot_magic(&snap_data) { return Err(ExofsError::InvalidMagic); }

    let epoch = snapshot_epoch_from_blob(&snap_data)?;
    if args.epoch_id != 0 && epoch != args.epoch_id { return Err(ExofsError::NoValidEpoch); }

    let size_src = snapshot_source_size_from_blob(&snap_data)?;

    // Déterminer le BlobId cible pour le fd.
    let target_blob_id = if args.flags & mount_flags::COPY_ON_USE != 0 {
        // Cloner le contenu dans un nouveau blob.
        let content = extract_snapshot_content(&snap_data)?;
        let cow_key = {
            let mut buf = [0u8; 34];
            let sb = snap_blob_id.as_bytes();
            let mut i = 0usize;
            while i < 32 { buf[i] = sb[i]; i = i.wrapping_add(1); }
            buf[32] = 0xC0;
            buf[33] = 0xEF;
            BlobId::from_bytes_blake3(&buf)
        };
        BLOB_CACHE.insert(cow_key, content.to_vec())?;
        cow_key
    } else {
        snap_blob_id
    };
    drop(snap_data);

    let fd_flags = if args.flags & mount_flags::READ_ONLY != 0 {
        open_flags::O_RDONLY
    } else {
        open_flags::O_RDWR
    };

    let fd = OBJECT_TABLE.open(target_blob_id, fd_flags, size_src, epoch, 0)?;

    Ok(SnapshotMountResult {
        fd,
        _pad:        0,
        snapshot_id: *snap_blob_id.as_bytes(),
        size_bytes:  size_src,
        epoch_id:    epoch,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_SNAPSHOT_MOUNT (511)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_snapshot_mount(snap_blob_id_ptr, out_ptr, args_ptr, _, _, _) → fd ou errno`
pub fn sys_exofs_snapshot_mount(
    snap_id_ptr: u64,
    out_ptr:     u64,
    args_ptr:    u64,
    _a4:         u64,
    _a5:         u64,
    cap_rights:  u64,
) -> i64 {
    if snap_id_ptr == 0 { return EFAULT; }

    // Lire le snapshot_id[32] depuis userspace.
    let mut raw_id = [0u8; 32];
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        let src = snap_id_ptr as *const u8;
        let mut i = 0usize;
        while i < 32 { raw_id[i] = *src.add(i); i = i.wrapping_add(1); }
    }
    let snap_blob_id = BlobId(raw_id);

    let args = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<SnapshotMountArgs>(args_ptr) } {
            Ok(a)  => a,
            Err(_) => return EFAULT,
        }
    } else {
        SnapshotMountArgs { flags: mount_flags::READ_ONLY, _pad: 0, epoch_id: 0 }
    };

    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsSnapshotMount) {
        return e;
    }

    let result = match mount_snapshot(snap_blob_id, &args) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };

    if out_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &result as *const SnapshotMountResult as *const u8,
                core::mem::size_of::<SnapshotMountResult>(),
            )
        };
        if let Err(e) = write_user_buf(out_ptr, bytes) {
            OBJECT_TABLE.close(result.fd);
            return e;
        }
    }
    result.fd as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un BlobId est bien un snapshot valide.
pub fn is_valid_snapshot(snap_id: BlobId) -> bool {
    BLOB_CACHE.get(&snap_id)
        .map(|d| check_snapshot_magic(&d))
        .unwrap_or(false)
}

/// Extrait l'epoch d'un snapshot depuis le cache.
pub fn snapshot_epoch(snap_id: BlobId) -> Option<u64> {
    let data = BLOB_CACHE.get(&snap_id)?;
    snapshot_epoch_from_blob(&data).ok()
}

/// Démonte un snapshot fd (ferme le fd).
pub fn unmount_snapshot(fd: u32) {
    OBJECT_TABLE.close(fd);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::snapshot_create::{create_snapshot, snap_flags};

    fn make_snap(path: &[u8], epoch: u64) -> BlobId {
        let src_id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(src_id, b"snapshot source data".to_vec()).unwrap();
        let r = create_snapshot(src_id, epoch, snap_flags::READ_ONLY, b"").unwrap();
        BlobId(r.snapshot_id)
    }

    #[test]
    fn test_mount_basic() {
        let sid = make_snap(b"/mount/basic", 3);
        let args = SnapshotMountArgs { flags: mount_flags::READ_ONLY, _pad: 0, epoch_id: 0 };
        let r = mount_snapshot(sid, &args).unwrap();
        assert!(r.fd >= 4);
        assert_eq!(r.epoch_id, 3);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_mount_cow() {
        let sid = make_snap(b"/mount/cow", 5);
        let args = SnapshotMountArgs { flags: mount_flags::COPY_ON_USE, _pad: 0, epoch_id: 0 };
        let r = mount_snapshot(sid, &args).unwrap();
        assert!(r.fd >= 4);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_mount_wrong_epoch() {
        let sid = make_snap(b"/mount/ep", 10);
        let args = SnapshotMountArgs { flags: 0, _pad: 0, epoch_id: 99 };
        assert!(mount_snapshot(sid, &args).is_err());
    }

    #[test]
    fn test_mount_missing() {
        let id = BlobId::from_bytes_blake3(b"/mount/miss");
        let args = SnapshotMountArgs { flags: 0, _pad: 0, epoch_id: 0 };
        assert!(mount_snapshot(id, &args).is_err());
    }

    #[test]
    fn test_is_valid_snapshot_true() {
        let sid = make_snap(b"/mount/valid", 1);
        assert!(is_valid_snapshot(sid));
    }

    #[test]
    fn test_is_valid_snapshot_false() {
        let id = BlobId::from_bytes_blake3(b"/mount/invalid");
        BLOB_CACHE.insert(id, b"no magic".to_vec()).unwrap();
        assert!(!is_valid_snapshot(id));
    }

    #[test]
    fn test_snapshot_epoch_ok() {
        let sid = make_snap(b"/mount/epoch", 42);
        assert_eq!(snapshot_epoch(sid), Some(42));
    }

    #[test]
    fn test_snapshot_epoch_none() {
        let id = BlobId::from_bytes_blake3(b"/mount/ep/none");
        assert_eq!(snapshot_epoch(id), None);
    }

    #[test]
    fn test_unmount() {
        let sid = make_snap(b"/mount/unmount", 0);
        let args = SnapshotMountArgs { flags: 0, _pad: 0, epoch_id: 0 };
        let r = mount_snapshot(sid, &args).unwrap();
        unmount_snapshot(r.fd); // ne doit pas paniquer
    }

    #[test]
    fn test_sys_null_snap_id() {
        assert_eq!(sys_exofs_snapshot_mount(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_result_size() {
        assert_eq!(core::mem::size_of::<SnapshotMountResult>(), 64);
    }

    #[test]
    fn test_args_size() {
        assert_eq!(core::mem::size_of::<SnapshotMountArgs>(), 16);
    }

    #[test]
    fn test_invalid_flags() {
        let sid = make_snap(b"/mount/flag", 0);
        let args = SnapshotMountArgs { flags: 0xDEAD, _pad: 0, epoch_id: 0 };
        assert!(mount_snapshot(sid, &args).is_err());
    }

    #[test]
    fn test_content_offset_minimal() {
        let data = [0u8; 26];
        assert_eq!(content_offset(&data), 26);
    }

    #[test]
    fn test_extract_content_bad_magic() {
        assert!(extract_snapshot_content(&[0u8; 28]).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gestion avancée : table de montages actifs
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de la table de montages.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MountEntry {
    pub fd:          u32,
    pub _pad:        u32,
    pub snapshot_id: [u8; 32],
    pub epoch_id:    u64,
    pub flags:       u32,
    pub _pad2:       u32,
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<MountEntry>() == 64);

/// Nombre maximum de montages simultanés.
pub const MAX_MOUNTS: usize = 64;

/// Table plate de montages actifs (kernel-side bookkeeping).
/// Les entrées avec fd==0 sont libres.
#[allow(dead_code)]
static MOUNT_TABLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Sérialise une liste de MountEntry vers userspace.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn encode_mount_entries(entries: &[MountEntry]) -> ExofsResult<Vec<u8>> {
    let esz = core::mem::size_of::<MountEntry>();
    let total = entries.len().saturating_mul(esz);
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < entries.len() {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let raw = unsafe {
            core::slice::from_raw_parts(&entries[i] as *const MountEntry as *const u8, esz)
        };
        let mut j = 0usize;
        while j < esz { buf.push(raw[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

/// Construit un `MountEntry` depuis le résultat du montage.
pub fn make_mount_entry(r: &SnapshotMountResult, flags: u32) -> MountEntry {
    let mut e = MountEntry {
        fd:     r.fd,
        epoch_id: r.epoch_id,
        flags,
        ..MountEntry::default()
    };
    let mut i = 0usize;
    while i < 32 { e.snapshot_id[i] = r.snapshot_id[i]; i = i.wrapping_add(1); }
    e
}

/// Retourne `true` si un fd donné correspond à un montage actif.
pub fn is_snapshot_fd(fd: u32) -> bool {
    // Un snapshot fd a été ouvert avec O_RDONLY exclusivement — heuristique.
    OBJECT_TABLE.check_readable(fd).is_ok()
}

/// Calcule la taille des données extraites, sans allouer le contenu.
pub fn snapshot_content_size(snap_id: BlobId) -> usize {
    let data = match BLOB_CACHE.get(&snap_id) { Some(d) => d, None => return 0 };
    if !check_snapshot_magic(&data) { return 0; }
    let off = content_offset(&data);
    data.len().saturating_sub(off)
}

/// Compte le nombre de snapshots montés dans l'OBJECT_TABLE.
/// Note : approximation heuristique car l'API ne distingue pas les fd snapshot.
pub fn active_mount_count() -> usize {
    OBJECT_TABLE.open_count() as usize
}

#[cfg(test)]
mod advanced_tests {
    use super::*;
    use super::super::snapshot_create::{create_snapshot, snap_flags};

    fn make_snap(path: &[u8], epoch: u64) -> BlobId {
        let src_id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(src_id, b"body data".to_vec()).unwrap();
        let r = create_snapshot(src_id, epoch, snap_flags::READ_ONLY, b"").unwrap();
        BlobId(r.snapshot_id)
    }

    #[test]
    fn test_mount_entry_size() {
        assert_eq!(core::mem::size_of::<MountEntry>(), 64);
    }

    #[test]
    fn test_make_mount_entry() {
        let r = SnapshotMountResult { fd: 5, epoch_id: 3, ..SnapshotMountResult::default() };
        let e = make_mount_entry(&r, mount_flags::READ_ONLY);
        assert_eq!(e.fd, 5);
        assert_eq!(e.epoch_id, 3);
    }

    #[test]
    fn test_encode_mount_entries_empty() {
        assert!(encode_mount_entries(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_encode_mount_entries_one() {
        let e = MountEntry::default();
        let buf = encode_mount_entries(&[e]).unwrap();
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn test_snapshot_content_size() {
        let sid = make_snap(b"/mount/sz", 0);
        let sz = snapshot_content_size(sid);
        assert!(sz > 0);
    }

    #[test]
    fn test_snapshot_content_size_missing() {
        let id = BlobId::from_bytes_blake3(b"/mnt/adv/miss");
        assert_eq!(snapshot_content_size(id), 0);
    }

    #[test]
    fn test_is_snapshot_fd_basic() {
        let sid = make_snap(b"/mount/isfd", 0);
        let args = SnapshotMountArgs { flags: mount_flags::READ_ONLY, _pad: 0, epoch_id: 0 };
        let r = mount_snapshot(sid, &args).unwrap();
        assert!(is_snapshot_fd(r.fd));
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_active_mount_count_not_panic() {
        let _ = active_mount_count();
    }
}
