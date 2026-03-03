//! object_set_meta.rs — SYS_EXOFS_OBJECT_SET_META (507)
//!
//! Gestion des métadonnées key/value attachées à un blob ExoFS.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT, EINVAL, ENOMEM,
    EXOFS_META_MAX, EXOFS_NAME_MAX,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'une clé de métadonnée (octets).
pub const META_KEY_MAX:   usize = 128;
/// Taille maximale d'une valeur de métadonnée (octets).
pub const META_VALUE_MAX: usize = EXOFS_META_MAX;
/// Nombre maximum d'entrées dans un bloc de métadonnées.
pub const META_ENTRIES_MAX: usize = 32;
/// Magic header d'un bloc de métadonnées ExoFS.
pub const META_MAGIC: u32 = 0xEF05_4D45; // "EF05ME"

// ─────────────────────────────────────────────────────────────────────────────
// Flags de métadonnées
// ─────────────────────────────────────────────────────────────────────────────

pub mod meta_flags {
    pub const SET:       u32 = 0x0001;
    pub const GET:       u32 = 0x0002;
    pub const DELETE:    u32 = 0x0004;
    pub const LIST:      u32 = 0x0008;
    pub const CLEAR_ALL: u32 = 0x0010;
    pub const VALID_MASK:u32 = SET | GET | DELETE | LIST | CLEAR_ALL;
}

// ─────────────────────────────────────────────────────────────────────────────
// Struct d'une entrée de métadonnées (format en-mémoire)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de métadonnées : clé + valeur, longueurs variables.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MetaEntry {
    pub key_len:   u16,
    pub value_len: u16,
    pub _pad:      u32,
    pub key:       [u8; META_KEY_MAX],
    pub value:     [u8; META_VALUE_MAX],
}

const _: () = assert!(META_KEY_MAX + META_VALUE_MAX + 8 <= 2048);

impl MetaEntry {
    pub fn new(key: &[u8], value: &[u8]) -> ExofsResult<Self> {
        if key.is_empty() || key.len() > META_KEY_MAX   { return Err(ExofsError::InvalidArgument); }
        if value.len() > META_VALUE_MAX                 { return Err(ExofsError::InvalidArgument); }
        let mut e = MetaEntry {
            key_len:   key.len()   as u16,
            value_len: value.len() as u16,
            _pad:      0,
            key:       [0u8; META_KEY_MAX],
            value:     [0u8; META_VALUE_MAX],
        };
        let mut i = 0usize;
        while i < key.len()   { e.key[i]   = key[i];   i = i.wrapping_add(1); }
        let mut j = 0usize;
        while j < value.len() { e.value[j] = value[j]; j = j.wrapping_add(1); }
        Ok(e)
    }

    pub fn key_bytes(&self) -> &[u8] { &self.key[..self.key_len as usize] }
    pub fn value_bytes(&self) -> &[u8] { &self.value[..self.value_len as usize] }

    pub fn key_eq(&self, k: &[u8]) -> bool {
        if self.key_len as usize != k.len() { return false; }
        let mut i = 0usize;
        while i < k.len() {
            if self.key[i] != k[i] { return false; }
            i = i.wrapping_add(1);
        }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bloc de métadonnées sérialisé dans le cache
// ─────────────────────────────────────────────────────────────────────────────
//
// Format binaire : magic(4) + count(4) + [MetaRaw]*count
// MetaRaw : key_len(2) + val_len(2) + key + value

/// En-tête sérialisée d'un bloc de métadonnées.
struct MetaBlockHeader {
    magic: u32,
    count: u32,
}

const RAW_HEADER_SIZE: usize = 8;

/// Clé interne du cache pour les métadonnées : Blake3(blob_id || b"\xFE\xED")
fn meta_blob_id(blob_id: BlobId) -> BlobId {
    let mut buf = [0u8; 34];
    let raw = blob_id.as_bytes();
    let mut i = 0usize;
    while i < 32 { buf[i] = raw[i]; i = i.wrapping_add(1); }
    buf[32] = 0xFE;
    buf[33] = 0xED;
    BlobId::from_bytes_blake3(&buf)
}

/// Désérialise les entrées depuis les octets du cache.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn deserialize_entries(raw: &[u8]) -> ExofsResult<Vec<MetaEntry>> {
    if raw.len() < RAW_HEADER_SIZE { return Err(ExofsError::CorruptedStructure); }
    let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if magic != META_MAGIC { return Err(ExofsError::InvalidMagic); }
    let count = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
    if count > META_ENTRIES_MAX { return Err(ExofsError::CorruptedStructure); }

    let mut entries: Vec<MetaEntry> = Vec::new();
    entries.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;

    let mut off = RAW_HEADER_SIZE;
    let mut i = 0usize;
    while i < count {
        if off.saturating_add(4) > raw.len() { break; }
        let kl = u16::from_le_bytes([raw[off], raw[off + 1]]) as usize;
        let vl = u16::from_le_bytes([raw[off + 2], raw[off + 3]]) as usize;
        off = off.saturating_add(4);
        if off.saturating_add(kl).saturating_add(vl) > raw.len() { break; }
        let k = &raw[off..off.saturating_add(kl)];
        off = off.saturating_add(kl);
        let v = &raw[off..off.saturating_add(vl)];
        off = off.saturating_add(vl);
        if let Ok(e) = MetaEntry::new(k, v) { entries.push(e); }
        i = i.wrapping_add(1);
    }
    Ok(entries)
}

/// Sérialise les entrées vers un Vec<u8>.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn serialize_entries(entries: &[MetaEntry]) -> ExofsResult<Vec<u8>> {
    let mut total = RAW_HEADER_SIZE;
    let mut i = 0usize;
    while i < entries.len() {
        total = total
            .saturating_add(4)
            .saturating_add(entries[i].key_len as usize)
            .saturating_add(entries[i].value_len as usize);
        i = i.wrapping_add(1);
    }
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    // Header
    let magic_le = META_MAGIC.to_le_bytes();
    let count_le = (entries.len() as u32).to_le_bytes();
    let mut j = 0usize;
    while j < 4 { buf.push(magic_le[j]); j = j.wrapping_add(1); }
    let mut k = 0usize;
    while k < 4 { buf.push(count_le[k]); k = k.wrapping_add(1); }
    // Entries
    let mut idx = 0usize;
    while idx < entries.len() {
        let e = &entries[idx];
        buf.push((e.key_len & 0xFF) as u8);
        buf.push((e.key_len >> 8)   as u8);
        buf.push((e.value_len & 0xFF) as u8);
        buf.push((e.value_len >> 8)   as u8);
        let mut ki = 0usize;
        while ki < e.key_len as usize   { buf.push(e.key[ki]);   ki = ki.wrapping_add(1); }
        let mut vi = 0usize;
        while vi < e.value_len as usize { buf.push(e.value[vi]); vi = vi.wrapping_add(1); }
        idx = idx.wrapping_add(1);
    }
    Ok(buf)
}

/// Charge les entrées du cache ou retourne un Vec vide.
fn load_entries(meta_id: BlobId) -> ExofsResult<Vec<MetaEntry>> {
    match BLOB_CACHE.get(&meta_id) {
        Some(data) => deserialize_entries(&data),
        None       => Ok(Vec::new()),
    }
}

/// Enregistre les entrées dans le cache.
fn save_entries(meta_id: BlobId, entries: &[MetaEntry]) -> ExofsResult<()> {
    let buf = serialize_entries(entries)?;
    BLOB_CACHE.insert(meta_id, &buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations de métadonnées
// ─────────────────────────────────────────────────────────────────────────────

pub fn meta_set(blob_id: BlobId, key: &[u8], value: &[u8]) -> ExofsResult<()> {
    let meta_id = meta_blob_id(blob_id);
    let mut entries = load_entries(meta_id)?;
    // Chercher et remplacer.
    let mut found = false;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].key_eq(key) {
            entries[i] = MetaEntry::new(key, value)?;
            found = true;
            break;
        }
        i = i.wrapping_add(1);
    }
    if !found {
        if entries.len() >= META_ENTRIES_MAX { return Err(ExofsError::QuotaExceeded); }
        entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        entries.push(MetaEntry::new(key, value)?);
    }
    save_entries(meta_id, &entries)
}

pub fn meta_get(blob_id: BlobId, key: &[u8], out: &mut Vec<u8>) -> ExofsResult<usize> {
    let meta_id = meta_blob_id(blob_id);
    let entries = load_entries(meta_id)?;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].key_eq(key) {
            let vl = entries[i].value_len as usize;
            out.try_reserve(vl).map_err(|_| ExofsError::NoMemory)?;
            let mut j = 0usize;
            while j < vl { out.push(entries[i].value[j]); j = j.wrapping_add(1); }
            return Ok(vl);
        }
        i = i.wrapping_add(1);
    }
    Err(ExofsError::ObjectNotFound)
}

pub fn meta_delete(blob_id: BlobId, key: &[u8]) -> ExofsResult<()> {
    let meta_id = meta_blob_id(blob_id);
    let mut entries = load_entries(meta_id)?;
    let mut new_entries: Vec<MetaEntry> = Vec::new();
    new_entries.try_reserve(entries.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < entries.len() {
        if !entries[i].key_eq(key) { new_entries.push(entries[i]); }
        i = i.wrapping_add(1);
    }
    save_entries(meta_id, &new_entries)
}

pub fn meta_clear(blob_id: BlobId) -> ExofsResult<()> {
    let meta_id = meta_blob_id(blob_id);
    BLOB_CACHE.invalidate(&meta_id);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_OBJECT_SET_META (507)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_set_meta(fd, key_ptr, key_len, val_ptr, val_len, flags) → 0 ou errno`
pub fn sys_exofs_object_set_meta(
    fd:      u64,
    key_ptr: u64,
    key_len: u64,
    val_ptr: u64,
    val_len: u64,
    flags:   u64,
) -> i64 {
    let f = flags as u32;
    if f & !meta_flags::VALID_MASK != 0 { return EINVAL; }

    let blob_id = match OBJECT_TABLE.blob_id_of(fd as u32) {
        Ok(id) => id,
        Err(e) => return exofs_err_to_errno(e),
    };

    if f & meta_flags::CLEAR_ALL != 0 {
        return match meta_clear(blob_id) {
            Ok(_)  => 0,
            Err(e) => exofs_err_to_errno(e),
        };
    }

    if f & meta_flags::SET != 0 {
        if key_ptr == 0 { return EFAULT; }
        let kl = key_len as usize;
        let vl = val_len as usize;
        if kl == 0 || kl > META_KEY_MAX   { return EINVAL; }
        if vl > META_VALUE_MAX             { return EINVAL; }
        let mut kbuf: Vec<u8> = Vec::new();
        kbuf.try_reserve(kl).map_err(|_| -12i64).unwrap_or_default();
        unsafe {
            let src = key_ptr as *const u8;
            let mut i = 0usize;
            while i < kl { kbuf.push(*src.add(i)); i = i.wrapping_add(1); }
        }
        let mut vbuf: Vec<u8> = Vec::new();
        if vl > 0 && val_ptr != 0 {
            vbuf.try_reserve(vl).map_err(|_| -12i64).unwrap_or_default();
            unsafe {
                let src = val_ptr as *const u8;
                let mut i = 0usize;
                while i < vl { vbuf.push(*src.add(i)); i = i.wrapping_add(1); }
            }
        }
        return match meta_set(blob_id, &kbuf, &vbuf) {
            Ok(_)  => 0,
            Err(e) => exofs_err_to_errno(e),
        };
    }

    EINVAL
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_blob(path: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, b"body").ok();
        id
    }

    #[test]
    fn test_meta_set_get() {
        let id = mk_blob(b"/meta/setget");
        meta_set(id, b"author", b"alice").unwrap();
        let mut out: Vec<u8> = Vec::new();
        let n = meta_get(id, b"author", &mut out).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&out, b"alice");
    }

    #[test]
    fn test_meta_overwrite() {
        let id = mk_blob(b"/meta/overwrite");
        meta_set(id, b"x", b"1").unwrap();
        meta_set(id, b"x", b"2").unwrap();
        let mut out: Vec<u8> = Vec::new();
        meta_get(id, b"x", &mut out).unwrap();
        assert_eq!(&out, b"2");
    }

    #[test]
    fn test_meta_delete() {
        let id = mk_blob(b"/meta/del");
        meta_set(id, b"k", b"v").unwrap();
        meta_delete(id, b"k").unwrap();
        let mut out: Vec<u8> = Vec::new();
        assert!(meta_get(id, b"k", &mut out).is_err());
    }

    #[test]
    fn test_meta_clear() {
        let id = mk_blob(b"/meta/clear");
        meta_set(id, b"a", b"1").unwrap();
        meta_set(id, b"b", b"2").unwrap();
        meta_clear(id).unwrap();
        let mut out: Vec<u8> = Vec::new();
        assert!(meta_get(id, b"a", &mut out).is_err());
    }

    #[test]
    fn test_meta_not_found() {
        let id = mk_blob(b"/meta/nf");
        let mut out: Vec<u8> = Vec::new();
        assert!(meta_get(id, b"missing", &mut out).is_err());
    }

    #[test]
    fn test_meta_entry_new_ok() {
        let e = MetaEntry::new(b"key", b"value").unwrap();
        assert_eq!(e.key_len, 3);
        assert_eq!(e.value_len, 5);
    }

    #[test]
    fn test_meta_entry_empty_key() {
        assert!(MetaEntry::new(b"", b"v").is_err());
    }

    #[test]
    fn test_meta_entry_key_eq() {
        let e = MetaEntry::new(b"hello", b"world").unwrap();
        assert!(e.key_eq(b"hello"));
        assert!(!e.key_eq(b"HELLO"));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let entries = [
            MetaEntry::new(b"k1", b"v1").unwrap(),
            MetaEntry::new(b"k2", b"v2").unwrap(),
        ];
        let raw = serialize_entries(&entries).unwrap();
        let back = deserialize_entries(&raw).unwrap();
        assert_eq!(back.len(), 2);
        assert!(back[0].key_eq(b"k1"));
    }

    #[test]
    fn test_meta_multiple_keys() {
        let id = mk_blob(b"/meta/multi");
        meta_set(id, b"name",    b"exofs").unwrap();
        meta_set(id, b"version", b"42").unwrap();
        let mut out: Vec<u8> = Vec::new();
        meta_get(id, b"version", &mut out).unwrap();
        assert_eq!(&out, b"42");
    }

    #[test]
    fn test_meta_blob_id_differs_from_source() {
        let id = mk_blob(b"/meta/differ");
        let mid = meta_blob_id(id);
        assert_ne!(id.as_bytes(), mid.as_bytes());
    }

    #[test]
    fn test_sys_null_fd_invalid() {
        let r = sys_exofs_object_set_meta(9999, 0, 0, 0, 0, meta_flags::SET as u64);
        assert!(r < 0);
    }

    #[test]
    fn test_sys_bad_flags() {
        assert_eq!(sys_exofs_object_set_meta(0, 0, 0, 0, 0, 0xDEAD), EINVAL);
    }
}
