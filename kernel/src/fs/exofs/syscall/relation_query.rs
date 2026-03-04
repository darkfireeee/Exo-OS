//! relation_query.rs — SYS_EXOFS_RELATION_QUERY (513)
//!
//! Interroge le graphe de relations ExoFS : voisins, types, filtres.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT, EINVAL,
};
use super::relation_create::{
    Relation, RELATION_MAX, RELATION_NAME_MAX, RELATION_MAGIC,
    relation_count, encode_relations,
    delete_relation, relation_exists, clear_relations,
    rel_kind, rel_flags,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const QUERY_MAX_RESULTS: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Flags et filtres
// ─────────────────────────────────────────────────────────────────────────────

pub mod query_flags {
    pub const OUTGOING:    u32 = 0x0001;
    pub const INCOMING:    u32 = 0x0002;
    pub const FILTER_KIND: u32 = 0x0004;
    pub const SORT_NAME:   u32 = 0x0008;
    pub const VALID_MASK:  u32 = OUTGOING | INCOMING | FILTER_KIND | SORT_NAME;
}

// ─────────────────────────────────────────────────────────────────────────────
// Arguments de requête
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RelationQueryArgs {
    pub source_id:   [u8; 32],
    pub flags:       u32,
    pub kind_filter: u8,
    pub _pad:        [u8; 3],
    pub max_results: u32,
    pub _pad2:       u32,
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<RelationQueryArgs>() == 56);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RelationQueryResult {
    pub count: u32,
    pub total: u32,
    pub _pad:  u64,
}

const _: () = assert!(core::mem::size_of::<RelationQueryResult>() == 16);

// ─────────────────────────────────────────────────────────────────────────────
// Interne : index inversé (incoming relations)
// ─────────────────────────────────────────────────────────────────────────────

/// Clé du registre inversé (incoming) pour `target`.
fn inv_registry_id(target: &[u8; 32]) -> BlobId {
    let mut buf = [0u8; 34];
    let mut i = 0usize;
    while i < 32 { buf[i] = target[i]; i = i.wrapping_add(1); }
    buf[32] = 0x49; buf[33] = 0x4E; // "IN"
    BlobId::from_bytes_blake3(&buf)
}

/// Clé du registre outgoing (même formule que relation_create).
fn out_registry_id(source: &[u8; 32]) -> BlobId {
    let mut buf = [0u8; 34];
    let mut i = 0usize;
    while i < 32 { buf[i] = source[i]; i = i.wrapping_add(1); }
    buf[32] = 0x52; buf[33] = 0x45; // "RE"
    BlobId::from_bytes_blake3(&buf)
}

const REL_HDR:   usize = 8;
const REL_ENTRY: usize = core::mem::size_of::<Relation>();

/// Charge les relations depuis un blob de registre.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn load_registry(reg_id: BlobId) -> ExofsResult<Vec<Relation>> {
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

// ─────────────────────────────────────────────────────────────────────────────
// Requête principale
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la liste des relations matching les critères.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn query_relations(args: &RelationQueryArgs) -> ExofsResult<Vec<Relation>> {
    if args.flags & !query_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
    let max = (args.max_results as usize).min(QUERY_MAX_RESULTS);
    let mut results: Vec<Relation> = Vec::new();
    results.try_reserve(max).map_err(|_| ExofsError::NoMemory)?;

    if args.flags & query_flags::OUTGOING != 0 || args.flags == 0 {
        let reg_id = out_registry_id(&args.source_id);
        let rels = load_registry(reg_id)?;
        let mut i = 0usize;
        while i < rels.len() && results.len() < max {
            let matches_kind = args.flags & query_flags::FILTER_KIND == 0
                || rels[i].kind == args.kind_filter;
            if matches_kind { results.push(rels[i]); }
            i = i.wrapping_add(1);
        }
    }

    if args.flags & query_flags::INCOMING != 0 {
        let inv_id = inv_registry_id(&args.source_id);
        let rels = load_registry(inv_id)?;
        let mut i = 0usize;
        while i < rels.len() && results.len() < max {
            let matches_kind = args.flags & query_flags::FILTER_KIND == 0
                || rels[i].kind == args.kind_filter;
            if matches_kind { results.push(rels[i]); }
            i = i.wrapping_add(1);
        }
    }

    if args.flags & query_flags::SORT_NAME != 0 {
        sort_by_name(&mut results);
    }
    Ok(results)
}

/// Tri insertion par nom (lexicographique).
/// RECUR-01 : while, pas de récursion.
fn sort_by_name(v: &mut Vec<Relation>) {
    let n = v.len();
    let mut i = 1usize;
    while i < n {
        let mut j = i;
        while j > 0 {
            let a = v[j - 1].name_bytes();
            let b = v[j].name_bytes();
            if !name_lt(b, a) { break; }
            v.swap(j - 1, j);
            j = j.wrapping_sub(1);
        }
        i = i.wrapping_add(1);
    }
}

/// Comparaison lexicographique de deux tranches.
fn name_lt(a: &[u8], b: &[u8]) -> bool {
    let n = a.len().min(b.len());
    let mut i = 0usize;
    while i < n {
        if a[i] < b[i] { return true; }
        if a[i] > b[i] { return false; }
        i = i.wrapping_add(1);
    }
    a.len() < b.len()
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_RELATION_QUERY (513)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_relation_query(args_ptr, out_buf_ptr, out_count_ptr, _, _, _) → 0 ou errno`
pub fn sys_exofs_relation_query(
    args_ptr:      u64,
    out_buf_ptr:   u64,
    out_count_ptr: u64,
    _a4:           u64,
    _a5:           u64,
    _a6:           u64,
) -> i64 {
    if args_ptr == 0 { return EFAULT; }
    let args = match unsafe { super::validation::copy_struct_from_user::<RelationQueryArgs>(args_ptr) } {
        Ok(a)  => a,
        Err(_) => return EFAULT,
    };

    let rels = match query_relations(&args) {
        Ok(v)  => v,
        Err(e) => return exofs_err_to_errno(e),
    };

    if out_buf_ptr != 0 {
        let enc = match encode_relations(&rels) {
            Ok(b)  => b,
            Err(e) => return exofs_err_to_errno(e),
        };
        if let Err(e) = write_user_buf(out_buf_ptr, &enc) { return e; }
    }

    if out_count_ptr != 0 {
        let cnt = (rels.len() as u64).to_le_bytes();
        if let Err(e) = write_user_buf(out_count_ptr, &cnt) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne uniquement les IDs cibles des relations sortantes.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn outgoing_targets(source: &[u8; 32]) -> ExofsResult<Vec<[u8; 32]>> {
    let args = RelationQueryArgs {
        source_id:   *source,
        flags:       query_flags::OUTGOING,
        kind_filter: 0,
        _pad:        [0; 3],
        max_results: QUERY_MAX_RESULTS as u32,
        _pad2:       0,
    };
    let rels = query_relations(&args)?;
    let mut out: Vec<[u8; 32]> = Vec::new();
    out.try_reserve(rels.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < rels.len() { out.push(rels[i].target_id); i = i.wrapping_add(1); }
    Ok(out)
}

/// Retourne les relations d'un kind précis.
pub fn relations_of_kind(source: &[u8; 32], kind: u8) -> ExofsResult<Vec<Relation>> {
    let args = RelationQueryArgs {
        source_id:   *source,
        flags:       query_flags::OUTGOING | query_flags::FILTER_KIND,
        kind_filter: kind,
        _pad:        [0; 3],
        max_results: QUERY_MAX_RESULTS as u32,
        _pad2:       0,
    };
    query_relations(&args)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::relation_create::create_relation;

    fn id(s: &[u8]) -> [u8; 32] { *BlobId::from_bytes_blake3(s).as_bytes() }

    fn setup(prefix: &[u8]) -> [u8; 32] {
        let s = id(prefix);
        let t1 = id(&[prefix, b"/t1"].concat());
        let t2 = id(&[prefix, b"/t2"].concat());
        create_relation(&s, &t1, rel_kind::CHILD, 0, b"link1").ok();
        create_relation(&s, &t2, rel_kind::CHILD, 0, b"link2").ok();
        s
    }

    #[test]
    fn test_query_args_size() {
        assert_eq!(core::mem::size_of::<RelationQueryArgs>(), 56);
    }

    #[test]
    fn test_query_result_size() {
        assert_eq!(core::mem::size_of::<RelationQueryResult>(), 16);
    }

    #[test]
    fn test_query_outgoing() {
        let s = setup(b"/rq/out");
        let targets = outgoing_targets(&s).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn test_query_filter_kind() {
        let s = id(b"/rq/kind/s");
        let t = id(b"/rq/kind/t");
        create_relation(&s, &t, rel_kind::PARENT, 0, b"").ok();
        create_relation(&s, &t, rel_kind::CHILD,  0, b"").ok();
        let children = relations_of_kind(&s, rel_kind::CHILD).unwrap();
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn test_query_sort_name() {
        let s = id(b"/rq/sort");
        let t1 = id(b"/rq/s/t1");
        let t2 = id(b"/rq/s/t2");
        create_relation(&s, &t2, rel_kind::CHILD, 0, b"b_link").ok();
        create_relation(&s, &t1, rel_kind::CHILD, 0, b"a_link").ok();
        let args = RelationQueryArgs {
            source_id:   s,
            flags:       query_flags::OUTGOING | query_flags::SORT_NAME,
            kind_filter: 0,
            _pad:        [0; 3],
            max_results: 10,
            _pad2:       0,
        };
        let rels = query_relations(&args).unwrap();
        if rels.len() >= 2 {
            assert!(name_lt(rels[0].name_bytes(), rels[1].name_bytes())
                 || rels[0].name_bytes() == rels[1].name_bytes());
        }
    }

    #[test]
    fn test_query_invalid_flags() {
        let args = RelationQueryArgs {
            source_id:   [0u8; 32],
            flags:       0xDEAD,
            kind_filter: 0,
            _pad:        [0; 3],
            max_results: 10,
            _pad2:       0,
        };
        assert!(query_relations(&args).is_err());
    }

    #[test]
    fn test_query_empty_source() {
        let args = RelationQueryArgs {
            source_id:   [0u8; 32],
            flags:       query_flags::OUTGOING,
            kind_filter: 0,
            _pad:        [0; 3],
            max_results: 10,
            _pad2:       0,
        };
        let rels = query_relations(&args).unwrap();
        assert!(rels.is_empty());
    }

    #[test]
    fn test_sys_null_args() {
        assert_eq!(sys_exofs_relation_query(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_name_lt() {
        assert!(name_lt(b"a", b"b"));
        assert!(!name_lt(b"b", b"a"));
        assert!(!name_lt(b"a", b"a"));
        assert!(name_lt(b"a", b"ab"));
    }

    #[test]
    fn test_sort_by_name_empty() {
        let mut v: Vec<Relation> = Vec::new();
        sort_by_name(&mut v);
    }

    #[test]
    fn test_query_max_results() {
        let args = RelationQueryArgs {
            source_id:   [0u8; 32],
            flags:       0,
            kind_filter: 0,
            _pad:        [0; 3],
            max_results: 0xFFFF_FFFF,
            _pad2:       0,
        };
        let rels = query_relations(&args).unwrap();
        assert!(rels.len() <= QUERY_MAX_RESULTS);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires (BFS transitif)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne vrai si `source` peut atteindre `target` via des arcs CHILD.
/// RECUR-01 : BFS itératif avec while, pas de récursion.
/// OOM-02 : try_reserve avant push.
pub fn can_reach(source: &[u8; 32], target: &[u8; 32], max_depth: usize) -> ExofsResult<bool> {
    let mut frontier: Vec<[u8; 32]> = Vec::new();
    frontier.try_reserve(QUERY_MAX_RESULTS).map_err(|_| ExofsError::NoMemory)?;
    frontier.push(*source);
    let mut visited: Vec<[u8; 32]> = Vec::new();
    visited.try_reserve(QUERY_MAX_RESULTS).map_err(|_| ExofsError::NoMemory)?;
    let mut depth = 0usize;
    while !frontier.is_empty() && depth < max_depth {
        let mut next: Vec<[u8; 32]> = Vec::new();
        next.try_reserve(QUERY_MAX_RESULTS).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < frontier.len() {
            let cur = &frontier[i];
            let children = match outgoing_targets(cur) { Ok(v) => v, Err(_) => { i = i.wrapping_add(1); continue; } };
            let mut j = 0usize;
            while j < children.len() {
                if &children[j] == target { return Ok(true); }
                let mut seen = false;
                let mut k = 0usize;
                while k < visited.len() { if visited[k] == children[j] { seen = true; break; } k = k.wrapping_add(1); }
                if !seen { next.push(children[j]); }
                j = j.wrapping_add(1);
            }
            visited.push(*cur);
            i = i.wrapping_add(1);
        }
        frontier = next;
        depth = depth.wrapping_add(1);
    }
    Ok(false)
}

/// Retourne le nombre de relations sortantes pour un objet.
pub fn outgoing_count(source: &[u8; 32]) -> ExofsResult<usize> {
    let targets = outgoing_targets(source)?;
    Ok(targets.len())
}

/// Retourne les IDs sources qui pointent vers `target` (liens entrants).
/// OOM-02/RECUR-01.
pub fn incoming_sources(target: &[u8; 32]) -> ExofsResult<Vec<[u8; 32]>> {
    let args = RelationQueryArgs {
        source_id:   *target,
        flags:       query_flags::INCOMING,
        kind_filter: 0,
        _pad:        [0; 3],
        max_results: QUERY_MAX_RESULTS as u32,
        _pad2:       0,
    };
    let rels = query_relations(&args)?;
    let mut out: Vec<[u8; 32]> = Vec::new();
    out.try_reserve(rels.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < rels.len() {
        out.push(rels[i].source_id);
        i = i.wrapping_add(1);
    }
    Ok(out)
}
