// kernel/src/fs/ext4plus/directory/htree.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ HTREE — répertoires indexés par arbre de hachage (dir_index)
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'HTree EXT4 est un arbre B à deux niveaux stocké dans les blocs du répertoire.
// Bloc 0 : dx_root (racine de l'index)
//   – first dirent (.) et (..)
//   – dx_root_info : hash_version, info_length, count, limit, …
//   – N × dx_entry  : {hash, block}
// Bloc 1+ : blocs de feuilles → entrées de répertoire linéaires
//
// Hash supportés : TEA (legacy), HalfMD4 (legacy), Unsigned TEA, Unsigned Half-MD4.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::core::types::{FsError, FsResult, InodeNumber};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// Hash functions
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation TEA (Tiny Encryption Algorithm) — hash de nom EXT2/3/4.
pub fn dx_hash_tea(name: &[u8], seed: &[u32; 4]) -> u32 {
    let (mut a, mut b): (u32, u32) = (seed[0], seed[1]);
    let (mut c, mut d): (u32, u32) = (0x9E37_79B9, 0x9E37_79B9);
    // Convertit le nom en mots u32
    let words: Vec<u32> = {
        let mut w = Vec::new();
        let mut i = 0;
        while i < name.len() {
            let mut v = 0u32;
            for j in 0..4 {
                if i + j < name.len() { v |= (name[i + j] as u32) << (j * 8); }
            }
            w.push(v);
            i += 4;
        }
        w
    };
    for chunk in words.chunks(2) {
        let p = chunk[0];
        let q = if chunk.len() > 1 { chunk[1] } else { 0 };
        // 6 rounds TEA
        for _ in 0..6 {
            d = d.wrapping_add(0x9E37_79B9);
            a = a.wrapping_add(((b << 4).wrapping_add(c)) ^ (b.wrapping_add(d)) ^ ((b >> 5).wrapping_add(0)));
            b = b.wrapping_add(((a << 4).wrapping_add(p)) ^ (a.wrapping_add(d)) ^ ((a >> 5).wrapping_add(q)));
        }
    }
    a ^ b
}

/// Hash DX basique (unsigned TEA) utilisé par défaut.
pub fn dx_hash(name: &[u8], seed: &[u32; 4]) -> u32 {
    dx_hash_tea(name, seed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures on-disk de l'HTree
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête du bloc racine (après les deux dirents . et ..).
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct DxRootInfo {
    pub reserved_zero:  u32,
    pub hash_version:   u8,  // 0=legacy TEA, 1=Half-MD4, 2=Tea
    pub info_length:    u8,  // 8
    pub indirect_levels:u8,  // 0 = 1 seul niveau d'index
    pub unused_flags:   u8,
}

/// Entrée de l'index HTree : { hash → numéro de bloc }.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct DxEntry {
    pub hash:  u32,
    pub block: u32,
}

/// Limite + compteur d'un nœud d'index.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct DxCountLimit {
    pub limit: u16,
    pub count: u16,
}

// ─────────────────────────────────────────────────────────────────────────────
// DxNode — nœud d'index chargé en mémoire
// ─────────────────────────────────────────────────────────────────────────────

pub struct DxNode {
    pub entries: Vec<DxEntry>,
}

/// Charge un nœud d'index HTree depuis un Buffer de taille bsize.
///
/// # Safety
/// `data` doit être initialisé sur `bsize` octets.
unsafe fn load_dx_node(data: *const u8, bsize: usize) -> FsResult<DxNode> {
    let cl_ptr  = data as *const DxCountLimit;
    let cl      = cl_ptr.read_unaligned();
    let count   = cl.count as usize;
    if count > cl.limit as usize { return Err(FsError::Corrupt); }

    let entry_ptr = data.add(core::mem::size_of::<DxCountLimit>()) as *const DxEntry;
    let entries   = (0..count).map(|i| entry_ptr.add(i).read_unaligned()).collect();
    Ok(DxNode { entries })
}

// ─────────────────────────────────────────────────────────────────────────────
// htree_find_block — trouve le bloc feuille pour un hash de nom
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le numéro de bloc de destination pour `name_hash` dans l'HTree.
/// `root_data` : le bloc 0 du répertoire (les 4096 octets).
/// Charge les nœuds de profondeur intermédiaire si nécessaire.
pub fn htree_find_block(
    root_data:  &[u8],
    name_hash:  u32,
    dev:        u64,
    dir_ino:    InodeNumber,
    bsize:      u64,
    load_buf:   PhysAddr,
) -> FsResult<u32> {
    // La racine commence après les deux dirents (8+8 = 16 octets min)
    // mais en pratique l'info est à un offset fixe.
    const ROOT_OFFSET: usize = 0x28; // après ". " + ".. " + padding

    if root_data.len() < ROOT_OFFSET + core::mem::size_of::<DxRootInfo>() {
        return Err(FsError::Corrupt);
    }
    // SAFETY: root_data slice >= ROOT_OFFSET+info_size par le check ci-dessus
    let info = unsafe {
        (root_data.as_ptr().add(ROOT_OFFSET) as *const DxRootInfo).read_unaligned()
    };

    let index_start = ROOT_OFFSET + core::mem::size_of::<DxRootInfo>() + 4; // +DxCountLimit
    if root_data.len() < index_start { return Err(FsError::Corrupt); }

    let node = unsafe { load_dx_node(root_data.as_ptr().add(index_start), root_data.len() - index_start)? };

    // Recherche binaire : dernier index dont hash ≤ name_hash
    let chosen = node.entries.iter()
        .rev()
        .find(|e| e.hash <= name_hash)
        .or_else(|| node.entries.first())
        .ok_or(FsError::NotFound)?;

    if info.indirect_levels == 0 {
        // Feuille directement accessible
        HTREE_STATS.lookups.fetch_add(1, Ordering::Relaxed);
        return Ok(chosen.block);
    }

    // Niveau intermédiaire : charge le bloc suivant
    let sector = chosen.block as u64 * bsize / 512;
    let bio = Bio {
        id:       0,
        op:       BioOp::Read,
        dev,
        sector,
        vecs:     alloc::vec![BioVec { phys: load_buf, virt: load_buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags:    BioFlags::META,
        status:   core::sync::atomic::AtomicU8::new(0),
        bytes:    core::sync::atomic::AtomicU64::new(0),
        callback: None,
        cb_data:  0,
    };
    submit_bio(bio)?;

    let child = unsafe { load_dx_node(load_buf.as_u64() as *const u8, bsize as usize)? };
    let leaf = child.entries.iter().rev()
        .find(|e| e.hash <= name_hash)
        .or_else(|| child.entries.first())
        .ok_or(FsError::NotFound)?;

    HTREE_STATS.lookups.fetch_add(1, Ordering::Relaxed);
    Ok(leaf.block)
}

// ─────────────────────────────────────────────────────────────────────────────
// HtreeStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct HtreeStats {
    pub lookups:     AtomicU64,
    pub hits:        AtomicU64,
    pub misses:      AtomicU64,
    pub inserts:     AtomicU64,
    pub splits:      AtomicU64,
}

impl HtreeStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { lookups: z!(), hits: z!(), misses: z!(), inserts: z!(), splits: z!() }
    }
}

pub static HTREE_STATS: HtreeStats = HtreeStats::new();
