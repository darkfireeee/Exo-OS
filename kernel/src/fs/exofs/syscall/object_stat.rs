//! object_stat.rs — SYS_EXOFS_OBJECT_STAT (506) — statistiques d'un objet ExoFS.
//!
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    read_user_path_heap, write_user_buf, exofs_err_to_errno, EFAULT, EINVAL,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Flags stat
// ─────────────────────────────────────────────────────────────────────────────

pub mod stat_flags {
    pub const USE_FD:        u32 = 0x0001;
    pub const NO_FOLLOW:     u32 = 0x0002;
    pub const INCLUDE_HASH:  u32 = 0x0004;
    pub const INCLUDE_EPOCH: u32 = 0x0008;
    pub const VALID_MASK:    u32 = USE_FD | NO_FOLLOW | INCLUDE_HASH | INCLUDE_EPOCH;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structure ObjectStat
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport complet sur un objet ExoFS — compatible ABI C.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectStat {
    /// Identifiant blob (clés du cache).
    pub blob_id:      [u8; 32],
    /// Identifiant objet (Blake3 XOR 0x5A).
    pub object_id:    [u8; 32],
    /// Taille en octets du contenu.
    pub size:         u64,
    /// Numéro d'époch courant.
    pub epoch_id:     u64,
    /// Nombre de références (hard links).
    pub link_count:   u32,
    /// Flags d'accès (open_flags).
    pub access_flags: u32,
    /// Type d'objet : 0=fichier, 1=répertoire, 2=lien, 3=snapshot.
    pub kind:         u8,
    /// Alignement.
    pub _pad:         [u8; 7],
    /// Hash du contenu (Blake3, 32 octets) — rempli si INCLUDE_HASH.
    pub content_hash: [u8; 32],
    /// UID propriétaire.
    pub owner_uid:    u64,
}

const _: () = assert!(core::mem::size_of::<ObjectStat>() == 176);

impl ObjectStat {
    /// Construit un stat depuis un BlobId + données du cache.
    fn from_blob(blob_id: BlobId, data: &[u8], epoch_id: u64, kind: u8) -> Self {
        let bid = blob_id.as_bytes();
        let mut obj_id = [0u8; 32];
        let mut i = 0usize;
        while i < 32 { obj_id[i] = bid[i] ^ 0x5A; i = i.wrapping_add(1); }

        let mut s = ObjectStat {
            size:         data.len() as u64,
            epoch_id,
            link_count:   1,
            access_flags: 0,
            kind,
            ..Self::default()
        };
        let mut j = 0usize;
        while j < 32 { s.blob_id[j] = bid[j]; j = j.wrapping_add(1); }
        let mut k = 0usize;
        while k < 32 { s.object_id[k] = obj_id[k]; k = k.wrapping_add(1); }
        s
    }

    /// Remplit le champ content_hash avec le Blake3 du contenu.
    fn fill_hash(&mut self, data: &[u8]) {
        let h = BlobId::from_bytes_blake3(data);
        let hb = h.as_bytes();
        let mut i = 0usize;
        while i < 32 { self.content_hash[i] = hb[i]; i = i.wrapping_add(1); }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stat par BlobId
// ─────────────────────────────────────────────────────────────────────────────

fn stat_blob(blob_id: BlobId, flags: u32) -> ExofsResult<ObjectStat> {
    let data = BLOB_CACHE.get(&blob_id)
        .ok_or(ExofsError::BlobNotFound)?;
    let mut s = ObjectStat::from_blob(blob_id, &data, 0, 0);
    if flags & stat_flags::INCLUDE_HASH != 0 { s.fill_hash(&data); }
    Ok(s)
}

// ─────────────────────────────────────────────────────────────────────────────
// Stat par fd
// ─────────────────────────────────────────────────────────────────────────────

fn stat_by_fd(fd: u32, flags: u32) -> ExofsResult<ObjectStat> {
    let entry = OBJECT_TABLE.get(fd)?;
    let blob_id = entry.blob_id;
    let data = BLOB_CACHE.get(&blob_id)
        .ok_or(ExofsError::BlobNotFound)?;
    let mut s = ObjectStat::from_blob(blob_id, &data, entry.epoch_id, 0);
    s.access_flags = entry.flags;
    s.owner_uid    = entry.owner_uid;
    if flags & stat_flags::INCLUDE_HASH != 0 { s.fill_hash(&data); }
    Ok(s)
}

// ─────────────────────────────────────────────────────────────────────────────
// Stat par chemin
// ─────────────────────────────────────────────────────────────────────────────

fn stat_by_path(path_bytes: &[u8], path_len: usize, flags: u32) -> ExofsResult<ObjectStat> {
    if path_len == 0 { return Err(ExofsError::InvalidArgument); }
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);
    stat_blob(blob_id, flags)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_OBJECT_STAT (506)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_stat(fd_or_path, is_fd_or_path_len, flags, out_ptr, _, _) → 0 ou errno`
///
/// Si `flags & USE_FD` : `fd_or_path` est un u32 fd.
/// Sinon : `fd_or_path` est un pointeur userspace vers une chaîne de chemin.
pub fn sys_exofs_object_stat(
    fd_or_path: u64,
    length:     u64,
    flags:      u64,
    out_ptr:    u64,
    _a5:        u64,
    _a6:        u64,
) -> i64 {
    if out_ptr == 0 { return EFAULT; }
    let f = flags as u32;
    if f & !stat_flags::VALID_MASK != 0 { return EINVAL; }

    let result = if f & stat_flags::USE_FD != 0 {
        let fd = fd_or_path as u32;
        match stat_by_fd(fd, f) {
            Ok(s)  => s,
            Err(e) => return exofs_err_to_errno(e),
        }
    } else {
        if fd_or_path == 0 { return EFAULT; }
        let mut path_buf: Vec<u8> = Vec::new();
        let actual_len = match read_user_path_heap(fd_or_path, &mut path_buf) {
            Ok(l)  => l,
            Err(e) => return e,
        };
        match stat_by_path(&path_buf, actual_len, f) {
            Ok(s)  => s,
            Err(e) => return exofs_err_to_errno(e),
        }
    };

    let bytes = unsafe {
        core::slice::from_raw_parts(
            &result as *const ObjectStat as *const u8,
            core::mem::size_of::<ObjectStat>(),
        )
    };
    match write_user_buf(out_ptr, bytes) {
        Ok(_)  => 0i64,
        Err(e) => e,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la taille logique d'un objet (0 si absent).
pub fn object_size(blob_id: &BlobId) -> u64 {
    BLOB_CACHE.get(blob_id).map(|d| d.len() as u64).unwrap_or(0)
}

/// Retourne le temps de dernière modification simulé (epoch_id fourni, 0 sinon).
pub fn object_epoch(blob_id: &BlobId) -> u64 {
    BLOB_CACHE.get(blob_id).map(|_| 0u64).unwrap_or(0)
}

/// Retourne vrai si l'objet est un répertoire (magic header).
pub fn is_directory(blob_id: &BlobId) -> bool {
    let data = match BLOB_CACHE.get(blob_id) {
        Some(d) => d,
        None    => return false,
    };
    if data.len() < 4 { return false; }
    data[0] == 0xCA && data[1] == 0xFE && data[2] == 0xD0 && data[3] == 0xD1
}

/// Construit un bloc compact de stats pour plusieurs blobs.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn stat_batch(blob_ids: &[BlobId], flags: u32) -> ExofsResult<Vec<ObjectStat>> {
    let mut out: Vec<ObjectStat> = Vec::new();
    out.try_reserve(blob_ids.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < blob_ids.len() {
        let s = stat_blob(blob_ids[i], flags).unwrap_or_default();
        out.push(s);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(path: &[u8], data: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, data).unwrap();
        id
    }

    #[test]
    fn test_stat_size_check() {
        assert_eq!(core::mem::size_of::<ObjectStat>(), 176);
    }

    #[test]
    fn test_stat_blob_basic() {
        let id = insert(b"/stat/basic", b"hello");
        let s = stat_blob(id, 0).unwrap();
        assert_eq!(s.size, 5);
    }

    #[test]
    fn test_stat_blob_not_found() {
        let id = BlobId::from_bytes_blake3(b"/stat/missing/obj");
        assert!(stat_blob(id, 0).is_err());
    }

    #[test]
    fn test_stat_with_hash() {
        let id = insert(b"/stat/hash", b"content");
        let s = stat_blob(id, stat_flags::INCLUDE_HASH).unwrap();
        assert_ne!(s.content_hash, [0u8; 32]);
    }

    #[test]
    fn test_stat_object_id_differs() {
        let id = insert(b"/stat/ids", b"xyz");
        let s = stat_blob(id, 0).unwrap();
        assert_ne!(s.blob_id, s.object_id);
    }

    #[test]
    fn test_stat_by_path_ok() {
        let path = b"/statpath/test";
        insert(path, b"data");
        let s = stat_by_path(path, path.len(), 0).unwrap();
        assert_eq!(s.size, 4);
    }

    #[test]
    fn test_stat_by_path_empty() {
        assert!(stat_by_path(b"", 0, 0).is_err());
    }

    #[test]
    fn test_object_size() {
        let id = insert(b"/objsize", b"12345678");
        assert_eq!(object_size(&id), 8);
    }

    #[test]
    fn test_is_directory_false() {
        let id = insert(b"/isdir/no", b"regular data");
        assert!(!is_directory(&id));
    }

    #[test]
    fn test_is_directory_true() {
        let id = BlobId::from_bytes_blake3(b"/isdir/yes");
        let hdr = [0xCAu8, 0xFE, 0xD0, 0xD1, 0, 0, 0, 0];
        BLOB_CACHE.insert(id, &hdr).unwrap();
        assert!(is_directory(&id));
    }

    #[test]
    fn test_stat_batch() {
        let ids = [
            insert(b"/batch/s1", b"a"),
            insert(b"/batch/s2", b"bb"),
        ];
        let results = stat_batch(&ids, 0).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].size, 2);
    }

    #[test]
    fn test_sys_stat_null_out() {
        assert_eq!(sys_exofs_object_stat(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_stat_invalid_flags() {
        assert_eq!(sys_exofs_object_stat(1, 0, 0xDEAD, 1, 0, 0), EINVAL);
    }

    #[test]
    fn test_stat_blob_from_constructor() {
        let id = BlobId::from_bytes_blake3(b"/stat/ctor");
        let data = b"test data bytes here";
        let s = ObjectStat::from_blob(id, data, 42, 1);
        assert_eq!(s.size, 20);
        assert_eq!(s.epoch_id, 42);
        assert_eq!(s.kind, 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires avancés : export/diff de stats
// ─────────────────────────────────────────────────────────────────────────────

/// Différence de taille entre deux versions du même objet.
/// ARITH-02 : saturating_sub.
pub fn size_delta(old: &ObjectStat, new_size: u64) -> i64 {
    let diff = new_size.saturating_sub(old.size);
    diff as i64
}

/// Retourne vrai si les deux stats font référence au même objet (blob_id identique).
pub fn same_object(a: &ObjectStat, b: &ObjectStat) -> bool {
    let mut eq = true;
    let mut i = 0usize;
    while i < 32 {
        if a.blob_id[i] != b.blob_id[i] { eq = false; break; }
        i = i.wrapping_add(1);
    }
    eq
}

/// Sérialise un `ObjectStat` en octets bruts.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn serialize_stat(s: &ObjectStat) -> ExofsResult<Vec<u8>> {
    let size = core::mem::size_of::<ObjectStat>();
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(size).map_err(|_| ExofsError::NoMemory)?;
    let raw = unsafe {
        core::slice::from_raw_parts(s as *const ObjectStat as *const u8, size)
    };
    let mut i = 0usize;
    while i < size {
        buf.push(raw[i]);
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

/// Désérialise un `ObjectStat` depuis un slice d'octets.
pub fn deserialize_stat(bytes: &[u8]) -> ExofsResult<ObjectStat> {
    let size = core::mem::size_of::<ObjectStat>();
    if bytes.len() < size { return Err(ExofsError::InvalidArgument); }
    let mut s = ObjectStat::default();
    let dst = unsafe {
        core::slice::from_raw_parts_mut(&mut s as *mut ObjectStat as *mut u8, size)
    };
    let mut i = 0usize;
    while i < size {
        dst[i] = bytes[i];
        i = i.wrapping_add(1);
    }
    Ok(s)
}

/// Compare le hash de contenu de deux stats.
/// Retourne `true` si identiques (contenu inchangé).
pub fn same_content(a: &ObjectStat, b: &ObjectStat) -> bool {
    let mut eq = true;
    let mut i = 0usize;
    while i < 32 {
        if a.content_hash[i] != b.content_hash[i] { eq = false; break; }
        i = i.wrapping_add(1);
    }
    eq
}

/// Retourne vrai si le stat indique que l'objet a été modifié (hash non nul).
pub fn is_content_known(s: &ObjectStat) -> bool {
    let mut nonzero = false;
    let mut i = 0usize;
    while i < 32 {
        if s.content_hash[i] != 0 { nonzero = true; break; }
        i = i.wrapping_add(1);
    }
    nonzero
}

// ─────────────────────────────────────────────────────────────────────────────
// Stat compact pour journalisation
// ─────────────────────────────────────────────────────────────────────────────

/// Version compacte d'ObjectStat pour le journal d'audit.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactStat {
    pub blob_id:  [u8; 32],
    pub size:     u64,
    pub epoch_id: u64,
    pub kind:     u8,
    pub _pad:     [u8; 7],
}

const _: () = assert!(core::mem::size_of::<CompactStat>() == 56);

impl CompactStat {
    pub fn from_full(s: &ObjectStat) -> Self {
        let mut c = Self {
            size:     s.size,
            epoch_id: s.epoch_id,
            kind:     s.kind,
            ..Self::default()
        };
        let mut i = 0usize;
        while i < 32 { c.blob_id[i] = s.blob_id[i]; i = i.wrapping_add(1); }
        c
    }
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    fn insert(path: &[u8], data: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, data).unwrap();
        id
    }

    #[test]
    fn test_size_delta_positive() {
        let s = ObjectStat { size: 100, ..ObjectStat::default() };
        assert_eq!(size_delta(&s, 150), 50);
    }

    #[test]
    fn test_size_delta_negative_clamped() {
        let s = ObjectStat { size: 200, ..ObjectStat::default() };
        // saturating_sub clamp à 0 donc delta = 0
        assert_eq!(size_delta(&s, 50), 0);
    }

    #[test]
    fn test_same_object_true() {
        let id = insert(b"/same/obj/a", b"x");
        let s1 = stat_blob(id, 0).unwrap();
        let s2 = stat_blob(id, 0).unwrap();
        assert!(same_object(&s1, &s2));
    }

    #[test]
    fn test_same_object_false() {
        let id1 = insert(b"/same/obj/b1", b"x");
        let id2 = insert(b"/same/obj/b2", b"y");
        let s1 = stat_blob(id1, 0).unwrap();
        let s2 = stat_blob(id2, 0).unwrap();
        assert!(!same_object(&s1, &s2));
    }

    #[test]
    fn test_serialize_deserialize() {
        let id = insert(b"/serde/stat", b"test123");
        let s = stat_blob(id, 0).unwrap();
        let bytes = serialize_stat(&s).unwrap();
        let s2 = deserialize_stat(&bytes).unwrap();
        assert_eq!(s.size, s2.size);
    }

    #[test]
    fn test_deserialize_too_short() {
        assert!(deserialize_stat(&[0u8; 10]).is_err());
    }

    #[test]
    fn test_compact_stat() {
        let id = insert(b"/compact/stat", b"abc");
        let s = stat_blob(id, 0).unwrap();
        let c = CompactStat::from_full(&s);
        assert_eq!(c.size, 3);
    }

    #[test]
    fn test_compact_stat_size() {
        assert_eq!(core::mem::size_of::<CompactStat>(), 56);
    }

    #[test]
    fn test_is_content_known_false() {
        let s = ObjectStat::default();
        assert!(!is_content_known(&s));
    }

    #[test]
    fn test_is_content_known_true() {
        let id = insert(b"/hash/known", b"data for hash");
        let s = stat_blob(id, stat_flags::INCLUDE_HASH).unwrap();
        assert!(is_content_known(&s));
    }

    #[test]
    fn test_same_content_identical() {
        let id = insert(b"/same/content", b"payload");
        let s = stat_blob(id, stat_flags::INCLUDE_HASH).unwrap();
        assert!(same_content(&s, &s));
    }

    #[test]
    fn test_same_content_different() {
        let id1 = insert(b"/diff/c1", b"aaa");
        let id2 = insert(b"/diff/c2", b"bbb");
        let s1 = stat_blob(id1, stat_flags::INCLUDE_HASH).unwrap();
        let s2 = stat_blob(id2, stat_flags::INCLUDE_HASH).unwrap();
        assert!(!same_content(&s1, &s2));
    }
}
