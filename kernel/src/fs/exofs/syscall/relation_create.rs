//! relation_create.rs — SYS_EXOFS_RELATION_CREATE (512)
//!
//! Crée une relation (arc orienté) entre deux objets ExoFS.
//! Les relations sont stockées dans un blob de registre dédié.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT, EINVAL,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const RELATION_MAGIC:   u32   = 0x52454C41; // "RELA"
pub const RELATION_MAX:     usize = 128;
pub const RELATION_NAME_MAX:usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Types de relation
// ─────────────────────────────────────────────────────────────────────────────

pub mod rel_kind {
    pub const HARDLINK:  u8 = 0;
    pub const SYMLINK:   u8 = 1;
    pub const PARENT:    u8 = 2;
    pub const CHILD:     u8 = 3;
    pub const SNAPSHOT:  u8 = 4;
    pub const REFERENCE: u8 = 5;
    pub const CUSTOM:    u8 = 6;
}

pub mod rel_flags {
    pub const BIDIRECTIONAL: u32 = 0x0001;
    pub const UNIQUE:        u32 = 0x0002;
    pub const PERSISTENT:    u32 = 0x0004;
    pub const VALID_MASK:    u32 = BIDIRECTIONAL | UNIQUE | PERSISTENT;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures
// ─────────────────────────────────────────────────────────────────────────────

/// Un arc dans le graphe des relations.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Relation {
    pub source_id: [u8; 32],
    pub target_id: [u8; 32],
    pub kind:      u8,
    pub _pad:      [u8; 7],
    pub flags:     u32,
    pub _pad2:     u32,
    pub name:      [u8; RELATION_NAME_MAX],
    pub name_len:  u8,
    pub _pad3:     [u8; 7],
}

const _: () = assert!(core::mem::size_of::<Relation>() <= 256);

impl Relation {
    pub fn new(
        src:   &[u8; 32],
        tgt:   &[u8; 32],
        kind:  u8,
        flags: u32,
        name:  &[u8],
    ) -> ExofsResult<Self> {
        if flags & !rel_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
        let nl = name.len().min(RELATION_NAME_MAX) as u8;
        let mut r = Relation { kind, flags, name_len: nl, ..Self::default() };
        let mut i = 0usize;
        while i < 32 { r.source_id[i] = src[i]; i = i.wrapping_add(1); }
        let mut j = 0usize;
        while j < 32 { r.target_id[j] = tgt[j]; j = j.wrapping_add(1); }
        let mut k = 0usize;
        while k < nl as usize { r.name[k] = name[k]; k = k.wrapping_add(1); }
        Ok(r)
    }

    pub fn name_bytes(&self) -> &[u8] { &self.name[..self.name_len as usize] }

    pub fn matches(&self, src: &[u8; 32], tgt: &[u8; 32]) -> bool {
        let mut se = true;
        let mut i = 0usize;
        while i < 32 { if self.source_id[i] != src[i] { se = false; break; } i = i.wrapping_add(1); }
        let mut te = true;
        let mut j = 0usize;
        while j < 32 { if self.target_id[j] != tgt[j] { te = false; break; } j = j.wrapping_add(1); }
        se && te
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stockage : blob registre de relations
// Format : magic(4) + count(4) + [Relation]*count
// ─────────────────────────────────────────────────────────────────────────────

const REL_HDR: usize = 8;
const REL_ENTRY: usize = core::mem::size_of::<Relation>();

/// Clé du registre de relations pour une source.
fn rel_registry_id(source: &[u8; 32]) -> BlobId {
    let mut buf = [0u8; 34];
    let mut i = 0usize;
    while i < 32 { buf[i] = source[i]; i = i.wrapping_add(1); }
    buf[32] = 0x52; buf[33] = 0x45; // "RE"
    BlobId::from_bytes_blake3(&buf)
}

/// Charge les relations depuis le cache.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn load_relations(reg_id: BlobId) -> ExofsResult<Vec<Relation>> {
    let data = match BLOB_CACHE.get(&reg_id) {
        Some(d) => d,
        None    => return Ok(Vec::new()),
    };
    if data.len() < REL_HDR { return Ok(Vec::new()); }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != RELATION_MAGIC { return Err(ExofsError::InvalidMagic); }
    let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let available = (data.len().saturating_sub(REL_HDR)) / REL_ENTRY;
    let n = count.min(available).min(RELATION_MAX);
    let mut rels: Vec<Relation> = Vec::new();
    rels.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n {
        let off = REL_HDR.saturating_add(i.saturating_mul(REL_ENTRY));
        let mut r = Relation::default();
        let dst = unsafe {
            core::slice::from_raw_parts_mut(&mut r as *mut Relation as *mut u8, REL_ENTRY)
        };
        let mut j = 0usize;
        while j < REL_ENTRY { dst[j] = data[off + j]; j = j.wrapping_add(1); }
        rels.push(r);
        i = i.wrapping_add(1);
    }
    Ok(rels)
}

/// Sauvegarde les relations dans le cache.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn save_relations(reg_id: BlobId, rels: &[Relation]) -> ExofsResult<()> {
    let total = REL_HDR.saturating_add(rels.len().saturating_mul(REL_ENTRY));
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let magic = RELATION_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(magic[i]); i = i.wrapping_add(1); }
    let cnt = (rels.len() as u32).to_le_bytes();
    let mut j = 0usize;
    while j < 4 { buf.push(cnt[j]); j = j.wrapping_add(1); }
    let mut k = 0usize;
    while k < rels.len() {
        let raw = unsafe {
            core::slice::from_raw_parts(&rels[k] as *const Relation as *const u8, REL_ENTRY)
        };
        let mut m = 0usize;
        while m < REL_ENTRY { buf.push(raw[m]); m = m.wrapping_add(1); }
        k = k.wrapping_add(1);
    }
    BLOB_CACHE.insert(reg_id, &buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations principales
// ─────────────────────────────────────────────────────────────────────────────

/// Crée une relation entre source et target.
pub fn create_relation(
    src:   &[u8; 32],
    tgt:   &[u8; 32],
    kind:  u8,
    flags: u32,
    name:  &[u8],
) -> ExofsResult<Relation> {
    let reg_id = rel_registry_id(src);
    let mut rels = load_relations(reg_id)?;
    if flags & rel_flags::UNIQUE != 0 {
        let mut i = 0usize;
        while i < rels.len() {
            if rels[i].matches(src, tgt) && rels[i].kind == kind {
                return Err(ExofsError::ObjectAlreadyExists);
            }
            i = i.wrapping_add(1);
        }
    }
    if rels.len() >= RELATION_MAX { return Err(ExofsError::QuotaExceeded); }
    let rel = Relation::new(src, tgt, kind, flags, name)?;
    rels.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
    rels.push(rel);
    save_relations(reg_id, &rels)?;
    if flags & rel_flags::BIDIRECTIONAL != 0 {
        let inv_reg = rel_registry_id(tgt);
        let mut inv_rels = load_relations(inv_reg)?;
        if inv_rels.len() < RELATION_MAX {
            let inv = Relation::new(tgt, src, kind, flags & !rel_flags::BIDIRECTIONAL, name)?;
            inv_rels.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            inv_rels.push(inv);
            save_relations(inv_reg, &inv_rels)?;
        }
    }
    Ok(rel)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_RELATION_CREATE (512)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RelationCreateArgs {
    pub source_id: [u8; 32],
    pub target_id: [u8; 32],
    pub kind:      u8,
    pub _pad:      [u8; 7],
    pub flags:     u32,
    pub _pad2:     u32,
    pub name_ptr:  u64,
    pub name_len:  u32,
    pub _pad3:     u32,
}

const _: () = assert!(core::mem::size_of::<RelationCreateArgs>() == 104);

/// `exofs_relation_create(args_ptr, out_ptr, _, _, _, _) → 0 ou errno`
pub fn sys_exofs_relation_create(
    args_ptr: u64,
    out_ptr:  u64,
    _a3:      u64,
    _a4:      u64,
    _a5:      u64,
    _a6:      u64,
) -> i64 {
    if args_ptr == 0 { return EFAULT; }
    let args = match unsafe { super::validation::copy_struct_from_user::<RelationCreateArgs>(args_ptr) } {
        Ok(a)  => a,
        Err(_) => return EFAULT,
    };
    let mut name_buf: Vec<u8> = Vec::new();
    if args.name_ptr != 0 && args.name_len > 0 {
        let nl = (args.name_len as usize).min(RELATION_NAME_MAX);
        name_buf.try_reserve(nl).unwrap_or(());
        unsafe {
            let src = args.name_ptr as *const u8;
            let mut i = 0usize;
            while i < nl { name_buf.push(*src.add(i)); i = i.wrapping_add(1); }
        }
    }
    let rel = match create_relation(&args.source_id, &args.target_id, args.kind, args.flags, &name_buf) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };
    if out_ptr != 0 {
        let bytes = unsafe {
            core::slice::from_raw_parts(&rel as *const Relation as *const u8, REL_ENTRY)
        };
        if let Err(e) = write_user_buf(out_ptr, bytes) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nombre de relations sortantes d'un objet.
pub fn relation_count(source: &[u8; 32]) -> usize {
    let reg_id = rel_registry_id(source);
    load_relations(reg_id).map(|v| v.len()).unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &[u8]) -> [u8; 32] { *BlobId::from_bytes_blake3(s).as_bytes() }

    #[test]
    fn test_create_basic() {
        let s = id(b"/rel/src1");
        let t = id(b"/rel/tgt1");
        let r = create_relation(&s, &t, rel_kind::HARDLINK, 0, b"").unwrap();
        assert_eq!(r.kind, rel_kind::HARDLINK);
    }

    #[test]
    fn test_create_unique_conflict() {
        let s = id(b"/rel/uniq/s");
        let t = id(b"/rel/uniq/t");
        create_relation(&s, &t, rel_kind::CHILD, rel_flags::UNIQUE, b"").unwrap();
        assert!(create_relation(&s, &t, rel_kind::CHILD, rel_flags::UNIQUE, b"").is_err());
    }

    #[test]
    fn test_relation_count() {
        let s = id(b"/rel/count");
        let t1 = id(b"/rel/t1");
        let t2 = id(b"/rel/t2");
        create_relation(&s, &t1, rel_kind::CHILD, 0, b"").unwrap();
        create_relation(&s, &t2, rel_kind::CHILD, 0, b"").unwrap();
        assert_eq!(relation_count(&s), 2);
    }

    #[test]
    fn test_bidirectional() {
        let s = id(b"/rel/bi/s");
        let t = id(b"/rel/bi/t");
        create_relation(&s, &t, rel_kind::PARENT, rel_flags::BIDIRECTIONAL, b"").unwrap();
        assert_eq!(relation_count(&t), 1);
    }

    #[test]
    fn test_invalid_flags() {
        let s = id(b"/rel/flag/s");
        let t = id(b"/rel/flag/t");
        assert!(create_relation(&s, &t, rel_kind::CUSTOM, 0xDEAD, b"").is_err());
    }

    #[test]
    fn test_relation_with_name() {
        let s = id(b"/rel/name/s");
        let t = id(b"/rel/name/t");
        let r = create_relation(&s, &t, rel_kind::REFERENCE, 0, b"mylink").unwrap();
        assert_eq!(r.name_bytes(), b"mylink");
    }

    #[test]
    fn test_sys_null_args() {
        assert_eq!(sys_exofs_relation_create(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_args_size() {
        assert_eq!(core::mem::size_of::<RelationCreateArgs>(), 104);
    }

    #[test]
    fn test_relation_matches() {
        let s = id(b"/rel/match/s");
        let t = id(b"/rel/match/t");
        let r = Relation::new(&s, &t, 0, 0, b"").unwrap();
        assert!(r.matches(&s, &t));
        assert!(!r.matches(&t, &s));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gestion avancée : suppression et vérification
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime une relation entre source et target par kind.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn delete_relation(src: &[u8; 32], tgt: &[u8; 32], kind: u8) -> ExofsResult<()> {
    let reg_id = rel_registry_id(src);
    let rels = load_relations(reg_id)?;
    let mut new_rels: Vec<Relation> = Vec::new();
    new_rels.try_reserve(rels.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < rels.len() {
        if !(rels[i].matches(src, tgt) && rels[i].kind == kind) { new_rels.push(rels[i]); }
        i = i.wrapping_add(1);
    }
    save_relations(reg_id, &new_rels)
}

/// Retourne `true` si une relation exacte existe.
pub fn relation_exists(src: &[u8; 32], tgt: &[u8; 32], kind: u8) -> bool {
    let reg_id = rel_registry_id(src);
    let rels = match load_relations(reg_id) {
        Ok(v)  => v,
        Err(_) => return false,
    };
    let mut i = 0usize;
    while i < rels.len() {
        if rels[i].matches(src, tgt) && rels[i].kind == kind { return true; }
        i = i.wrapping_add(1);
    }
    false
}

/// Supprime toutes les relations sortantes d'un objet.
pub fn clear_relations(src: &[u8; 32]) -> ExofsResult<()> {
    let reg_id = rel_registry_id(src);
    BLOB_CACHE.invalidate(&reg_id);
    Ok(())
}

/// Sérialise une liste de `Relation` en octets.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn encode_relations(rels: &[Relation]) -> ExofsResult<Vec<u8>> {
    let total = rels.len().saturating_mul(REL_ENTRY);
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < rels.len() {
        let raw = unsafe {
            core::slice::from_raw_parts(&rels[i] as *const Relation as *const u8, REL_ENTRY)
        };
        let mut j = 0usize;
        while j < REL_ENTRY { buf.push(raw[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    fn id(s: &[u8]) -> [u8; 32] { *BlobId::from_bytes_blake3(s).as_bytes() }

    #[test]
    fn test_delete_relation() {
        let s = id(b"/rdel/s");
        let t = id(b"/rdel/t");
        create_relation(&s, &t, rel_kind::CHILD, 0, b"").unwrap();
        assert_eq!(relation_count(&s), 1);
        delete_relation(&s, &t, rel_kind::CHILD).unwrap();
        assert_eq!(relation_count(&s), 0);
    }

    #[test]
    fn test_relation_exists_true() {
        let s = id(b"/rex/s");
        let t = id(b"/rex/t");
        create_relation(&s, &t, rel_kind::PARENT, 0, b"").unwrap();
        assert!(relation_exists(&s, &t, rel_kind::PARENT));
    }

    #[test]
    fn test_relation_exists_false() {
        let s = id(b"/rex2/s");
        let t = id(b"/rex2/t");
        assert!(!relation_exists(&s, &t, rel_kind::CHILD));
    }

    #[test]
    fn test_clear_relations() {
        let s = id(b"/rclear/s");
        let t = id(b"/rclear/t");
        create_relation(&s, &t, rel_kind::CHILD, 0, b"").unwrap();
        clear_relations(&s).unwrap();
        assert_eq!(relation_count(&s), 0);
    }

    #[test]
    fn test_encode_relations() {
        let s = id(b"/renc/s");
        let t = id(b"/renc/t");
        let r = create_relation(&s, &t, rel_kind::CUSTOM, 0, b"foo").unwrap();
        let buf = encode_relations(&[r]).unwrap();
        assert_eq!(buf.len(), REL_ENTRY);
    }

    #[test]
    fn test_encode_empty() {
        assert!(encode_relations(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_quota_exceeded() {
        let s = id(b"/rq/s");
        let mut ok = true;
        let mut i = 0usize;
        while i < RELATION_MAX {
            let t_bytes = (i as u64).to_le_bytes();
            let mut tid = [0u8; 32];
            let mut j = 0usize;
            while j < 8 { tid[j] = t_bytes[j]; j = j.wrapping_add(1); }
            if create_relation(&s, &tid, rel_kind::CHILD, 0, b"").is_err() { ok = false; break; }
            i = i.wrapping_add(1);
        }
        assert!(create_relation(&s, &[0xFF; 32], rel_kind::CHILD, 0, b"").is_err());
    }
}
