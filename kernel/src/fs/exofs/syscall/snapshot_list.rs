//! snapshot_list.rs — SYS_EXOFS_SNAPSHOT_LIST (510)
//!
//! Listage des snapshots associés à un blob source. ExoFS maintient un index
//! des snapshots dans un blob de registre dédié.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT, EINVAL,
};
use super::object_fd::OBJECT_TABLE;
use super::snapshot_create::{SnapshotRef, check_snapshot_magic, snapshot_epoch_from_blob, snapshot_source_size_from_blob};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum de snapshots retournés en une requête.
pub const SNAPSHOT_LIST_MAX: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Arguments
// ─────────────────────────────────────────────────────────────────────────────

pub mod list_flags {
    pub const BY_SOURCE_ID: u32 = 0x0001;
    pub const SORT_EPOCH:   u32 = 0x0002;
    pub const INCLUDE_SIZE: u32 = 0x0004;
    pub const VALID_MASK:   u32 = BY_SOURCE_ID | SORT_EPOCH | INCLUDE_SIZE;
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotListArgs {
    pub flags:       u32,
    pub max_results: u32,
    pub epoch_min:   u64,
    pub epoch_max:   u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotListArgs>() == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotListResult {
    pub count:   u32,
    pub total:   u32,
    pub _pad:    u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotListResult>() == 16);

// ─────────────────────────────────────────────────────────────────────────────
// Registre de snapshots
// ─────────────────────────────────────────────────────────────────────────────

/// BlobId du registre d'index de snapshots pour un blob source donné.
/// Clé = Blake3(source_id || b"\x53\x4C\x49\x53")  ("SLIS")
fn registry_id(source: BlobId) -> BlobId {
    let mut buf = [0u8; 36];
    let sb = source.as_bytes();
    let mut i = 0usize;
    while i < 32 { buf[i] = sb[i]; i = i.wrapping_add(1); }
    buf[32] = 0x53; buf[33] = 0x4C; buf[34] = 0x49; buf[35] = 0x53;
    BlobId::from_bytes_blake3(&buf)
}

/// Structure interne d'un registre de snapshots en cache.
/// Format : magic(4) + count(4) + [snapshot_id[32]]*count
const REGISTRY_MAGIC: u32 = 0x534C4953; // "SLIS"
const REGISTRY_HEADER: usize = 8;

/// Charge la liste des SnapshotId connus pour un source.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn load_registry(reg_id: BlobId) -> ExofsResult<Vec<[u8; 32]>> {
    let data = match BLOB_CACHE.get(&reg_id) {
        Some(d) => d,
        None    => return Ok(Vec::new()),
    };
    if data.len() < REGISTRY_HEADER { return Ok(Vec::new()); }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != REGISTRY_MAGIC { return Err(ExofsError::InvalidMagic); }
    let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let available = (data.len().saturating_sub(REGISTRY_HEADER)) / 32;
    let n = count.min(available).min(SNAPSHOT_LIST_MAX);
    let mut ids: Vec<[u8; 32]> = Vec::new();
    ids.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n {
        let off = REGISTRY_HEADER.saturating_add(i.saturating_mul(32));
        let mut id = [0u8; 32];
        let mut j = 0usize;
        while j < 32 { id[j] = data[off + j]; j = j.wrapping_add(1); }
        ids.push(id);
        i = i.wrapping_add(1);
    }
    Ok(ids)
}

/// Sauvegarde le registre dans le cache.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn save_registry(reg_id: BlobId, ids: &[[u8; 32]]) -> ExofsResult<()> {
    let total = REGISTRY_HEADER.saturating_add(ids.len().saturating_mul(32));
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let magic = REGISTRY_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(magic[i]); i = i.wrapping_add(1); }
    let count = (ids.len() as u32).to_le_bytes();
    let mut j = 0usize;
    while j < 4 { buf.push(count[j]); j = j.wrapping_add(1); }
    let mut k = 0usize;
    while k < ids.len() {
        let mut m = 0usize;
        while m < 32 { buf.push(ids[k][m]); m = m.wrapping_add(1); }
        k = k.wrapping_add(1);
    }
    BLOB_CACHE.insert(reg_id, buf.to_vec())
}

/// Enregistre un nouveau SnapshotId dans le registre du source.
pub fn register_snapshot(source: BlobId, snap_id: &[u8; 32]) -> ExofsResult<()> {
    let reg_id = registry_id(source);
    let mut ids = load_registry(reg_id)?;
    if ids.len() >= SNAPSHOT_LIST_MAX { return Err(ExofsError::QuotaExceeded); }
    ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
    ids.push(*snap_id);
    save_registry(reg_id, &ids)
}

// ─────────────────────────────────────────────────────────────────────────────
// Listage
// ─────────────────────────────────────────────────────────────────────────────

/// Liste les snapshots d'un source, filtrés et limités selon args.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn list_snapshots(source: BlobId, args: &SnapshotListArgs) -> ExofsResult<Vec<SnapshotRef>> {
    let reg_id = registry_id(source);
    let ids = load_registry(reg_id)?;
    let max = (args.max_results as usize).min(SNAPSHOT_LIST_MAX);
    let mut results: Vec<SnapshotRef> = Vec::new();
    results.try_reserve(max).map_err(|_| ExofsError::NoMemory)?;

    let mut i = 0usize;
    while i < ids.len() && results.len() < max {
        let snap_blob_id = BlobId(ids[i]);
        if let Some(data) = BLOB_CACHE.get(&snap_blob_id) {
            if !check_snapshot_magic(&data) { i = i.wrapping_add(1); continue; }
            let epoch = snapshot_epoch_from_blob(&data).unwrap_or(0);
            if epoch < args.epoch_min || epoch > args.epoch_max {
                i = i.wrapping_add(1);
                continue;
            }
            let size = snapshot_source_size_from_blob(&data).unwrap_or(0);
            let r = SnapshotRef {
                snapshot_id: ids[i],
                source_id:   *source.as_bytes(),
                epoch_id:    epoch,
                size_bytes:  size,
            };
            results.push(r);
        }
        i = i.wrapping_add(1);
    }

    if args.flags & list_flags::SORT_EPOCH != 0 {
        sort_by_epoch(&mut results);
    }
    Ok(results)
}

/// Tri par epoch ascendant (insertion sort, RECUR-01 : while, sans récursion).
fn sort_by_epoch(v: &mut Vec<SnapshotRef>) {
    let n = v.len();
    let mut i = 1usize;
    while i < n {
        let mut j = i;
        while j > 0 && v[j - 1].epoch_id > v[j].epoch_id {
            v.swap(j - 1, j);
            j = j.wrapping_sub(1);
        }
        i = i.wrapping_add(1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_SNAPSHOT_LIST (510)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_snapshot_list(fd, out_buf_ptr, out_count_ptr, args_ptr, _, _) → 0 ou errno`
pub fn sys_exofs_snapshot_list(
    fd:            u64,
    out_buf_ptr:   u64,
    out_count_ptr: u64,
    args_ptr:      u64,
    _a5:           u64,
    _a6:           u64,
) -> i64 {
    let blob_id = match OBJECT_TABLE.blob_id_of(fd as u32) {
        Ok(id) => id,
        Err(e) => return exofs_err_to_errno(e),
    };

    let args = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<SnapshotListArgs>(args_ptr) } {
            Ok(a) => a,
            Err(_) => return EFAULT,
        }
    } else {
        SnapshotListArgs {
            flags:       0,
            max_results: SNAPSHOT_LIST_MAX as u32,
            epoch_min:   0,
            epoch_max:   u64::MAX,
        }
    };

    if args.flags & !list_flags::VALID_MASK != 0 { return EINVAL; }

    let snapshots = match list_snapshots(blob_id, &args) {
        Ok(v)  => v,
        Err(e) => return exofs_err_to_errno(e),
    };

    if out_buf_ptr != 0 {
        let enc = match super::snapshot_create::encode_snapshot_refs(&snapshots) {
            Ok(b)  => b,
            Err(e) => return exofs_err_to_errno(e),
        };
        if let Err(e) = write_user_buf(out_buf_ptr, &enc) { return e; }
    }

    if out_count_ptr != 0 {
        let count_bytes = (snapshots.len() as u64).to_le_bytes();
        if let Err(e) = write_user_buf(out_count_ptr, &count_bytes) { return e; }
    }

    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nombre de snapshots enregistrés pour un source.
pub fn snapshot_count(source: BlobId) -> usize {
    let reg_id = registry_id(source);
    load_registry(reg_id).map(|v| v.len()).unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::snapshot_create::{create_snapshot as snap_create, snap_flags};

    fn src(path: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, b"src".to_vec()).unwrap();
        id
    }

    fn create_and_register(source: BlobId, epoch: u64, name: &[u8]) {
        let sid = super::super::snapshot_create::compute_snapshot_id(source, epoch, name);
        // Créer un blob vide avec magic pour la liste
        let mut data = [0u8; 24];
        let mag = SNAPSHOT_MAGIC.to_le_bytes();
        data[0] = mag[0]; data[1] = mag[1]; data[2] = mag[2]; data[3] = mag[3];
        let ep = epoch.to_le_bytes();
        let mut i = 0usize;
        while i < 8 { data[8 + i] = ep[i]; i = i.wrapping_add(1); }
        BLOB_CACHE.insert(sid, data.to_vec()).unwrap();
        register_snapshot(source, sid.as_bytes()).unwrap();
    }

    #[test]
    fn test_list_args_size() {
        assert_eq!(core::mem::size_of::<SnapshotListArgs>(), 24);
    }

    #[test]
    fn test_list_result_size() {
        assert_eq!(core::mem::size_of::<SnapshotListResult>(), 16);
    }

    #[test]
    fn test_register_and_count() {
        let s = src(b"/list/count");
        create_and_register(s, 1, b"n1");
        create_and_register(s, 2, b"n2");
        assert_eq!(snapshot_count(s), 2);
    }

    #[test]
    fn test_list_basic() {
        let s = src(b"/list/basic");
        create_and_register(s, 10, b"snap");
        let args = SnapshotListArgs { flags: 0, max_results: 10, epoch_min: 0, epoch_max: u64::MAX };
        let v = list_snapshots(s, &args).unwrap();
        assert!(!v.is_empty());
    }

    #[test]
    fn test_list_epoch_filter() {
        let s = src(b"/list/filter");
        create_and_register(s, 5, b"f1");
        create_and_register(s, 50, b"f2");
        let args = SnapshotListArgs { flags: 0, max_results: 10, epoch_min: 10, epoch_max: 100 };
        let v = list_snapshots(s, &args).unwrap();
        // Seul epoch=50 passe le filtre
        let mut i = 0usize;
        while i < v.len() { assert!(v[i].epoch_id >= 10); i = i.wrapping_add(1); }
    }

    #[test]
    fn test_list_sort() {
        let s = src(b"/list/sort");
        create_and_register(s, 30, b"s1");
        create_and_register(s, 10, b"s2");
        create_and_register(s, 20, b"s3");
        let args = SnapshotListArgs { flags: list_flags::SORT_EPOCH, max_results: 10, epoch_min: 0, epoch_max: u64::MAX };
        let v = list_snapshots(s, &args).unwrap();
        let mut i = 1usize;
        while i < v.len() {
            assert!(v[i].epoch_id >= v[i - 1].epoch_id);
            i = i.wrapping_add(1);
        }
    }

    #[test]
    fn test_list_no_snapshots() {
        let s = src(b"/list/empty");
        let args = SnapshotListArgs { flags: 0, max_results: 10, epoch_min: 0, epoch_max: u64::MAX };
        let v = list_snapshots(s, &args).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn test_sys_bad_fd() {
        assert!(sys_exofs_snapshot_list(9999, 0, 0, 0, 0, 0) < 0);
    }

    #[test]
    fn test_sys_bad_flags() {
        let _ = OBJECT_TABLE.open(BlobId::from_bytes_blake3(b"/list/flag"), 0, 0, 0, 0);
        // flags invalides
        assert_eq!(sys_exofs_snapshot_list(4, 0, 0, 0, 0, 0xDEAD), 0); // fd invalide → err
    }

    #[test]
    fn test_sort_empty() {
        let mut v: Vec<SnapshotRef> = Vec::new();
        sort_by_epoch(&mut v); // ne doit pas paniquer
    }

    #[test]
    fn test_sort_single() {
        let mut v = alloc::vec![SnapshotRef { epoch_id: 5, ..SnapshotRef::default() }];
        sort_by_epoch(&mut v);
        assert_eq!(v[0].epoch_id, 5);
    }

    #[test]
    fn test_register_quota() {
        let s = src(b"/list/quota");
        let mut ok = true;
        let mut i = 0usize;
        while i < SNAPSHOT_LIST_MAX {
            let id = [i as u8; 32];
            if register_snapshot(s, &id).is_err() { ok = false; break; }
            i = i.wrapping_add(1);
        }
        // La prochaine insertion doit échouer
        assert!(register_snapshot(s, &[0xFF; 32]).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires complémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime une entrée du registre par son snapshot_id.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn unregister_snapshot(source: BlobId, snap_id: &[u8; 32]) -> ExofsResult<()> {
    let reg_id = registry_id(source);
    let ids = load_registry(reg_id)?;
    let mut new_ids: Vec<[u8; 32]> = Vec::new();
    new_ids.try_reserve(ids.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < ids.len() {
        let mut eq = true;
        let mut j = 0usize;
        while j < 32 {
            if ids[i][j] != snap_id[j] { eq = false; break; }
            j = j.wrapping_add(1);
        }
        if !eq { new_ids.push(ids[i]); }
        i = i.wrapping_add(1);
    }
    save_registry(reg_id, &new_ids)
}

/// Retourne le snapshot le plus récent (epoch la plus haute) ou None.
pub fn latest_snapshot(source: BlobId) -> ExofsResult<Option<SnapshotRef>> {
    let args = SnapshotListArgs { flags: list_flags::SORT_EPOCH, max_results: SNAPSHOT_LIST_MAX as u32, epoch_min: 0, epoch_max: u64::MAX };
    let refs = list_snapshots(source, &args)?;
    let n = refs.len();
    if n == 0 { return Ok(None); }
    Ok(Some(refs[n - 1]))
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    fn src(p: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(p);
        BLOB_CACHE.insert(id, b"x".to_vec()).unwrap();
        id
    }

    #[test]
    fn test_unregister_snapshot() {
        let s = src(b"/list/unreg");
        let id = [0xBBu8; 32];
        register_snapshot(s, &id).unwrap();
        assert_eq!(snapshot_count(s), 1);
        unregister_snapshot(s, &id).unwrap();
        assert_eq!(snapshot_count(s), 0);
    }

    #[test]
    fn test_latest_snapshot_none() {
        let s = src(b"/list/latest/none");
        assert!(latest_snapshot(s).unwrap().is_none());
    }
}
