// kernel/src/fs/ext4plus/inode/xattr.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ EXTENDED ATTRIBUTES — xattr inline + block (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Les attributs étendus (xattr) peuvent être stockés :
//   1. En inline dans le corps de l'inode (extra isize) — EXT4_XATTR_MAGIC inline.
//   2. Dans un bloc séparé pointé par i_file_acl.
//
// Format d'entrée xattr (Ext4XattrEntry, variable-length) :
//   e_name_len    u8   — longueur du nom (sans namespace prefix)
//   e_name_index  u8   — index du namespace (1=user, 2=posix_acl_access, etc.)
//   e_value_offs  u16  — offset de la valeur depuis le début du bloc
//   e_value_inum  u32  — inode du bloc valeur (large xattr, ≥ Linux 4.13)
//   e_value_size  u32  — taille de la valeur en octets
//   e_hash        u32  — hash du nom+valeur
//   e_name        [u8] — nom (e_name_len octets)
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::string::String;

use crate::fs::core::types::{FsError, FsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes et namespaces
// ─────────────────────────────────────────────────────────────────────────────

pub const EXT4_XATTR_MAGIC:   u32 = 0xEA020000;
pub const EXT4_XATTR_MIN_HASH_SHIFT: u32 = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum XattrNamespace {
    User           = 1,
    PosixAclAccess = 2,
    PosixAclDefault= 3,
    Trusted        = 4,
    Lustre         = 5,
    Security       = 6,
    System         = 7,
    RichAcl        = 8,
    Unknown        = 0xFF,
}

impl XattrNamespace {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::User, 2 => Self::PosixAclAccess, 3 => Self::PosixAclDefault,
            4 => Self::Trusted, 5 => Self::Lustre, 6 => Self::Security,
            7 => Self::System, 8 => Self::RichAcl, _ => Self::Unknown,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ext4XattrEntry — entrée on-disk (variable length)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Ext4XattrEntryHeader {
    pub e_name_len:   u8,
    pub e_name_index: u8,
    pub e_value_offs: u16,
    pub e_value_inum: u32,
    pub e_value_size: u32,
    pub e_hash:       u32,
}

pub const XATTR_ENTRY_HDR_SIZE: usize = core::mem::size_of::<Ext4XattrEntryHeader>();

/// Représentation en mémoire d'un attribut étendu.
#[derive(Clone)]
pub struct XattrEntry {
    pub namespace: XattrNamespace,
    pub name:      Vec<u8>,
    pub value:     Vec<u8>,
    pub hash:      u32,
}

impl XattrEntry {
    /// Calcule le hash EXT4 (nom + valeur).
    pub fn compute_hash(&self) -> u32 {
        let mut h: u32 = 0;
        for &b in &self.name  { h = h.wrapping_mul(16) ^ (b as u32); }
        for &b in &self.value { h = h.wrapping_mul(16) ^ (b as u32); }
        h
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XattrBlock — analyse d'un bloc xattr complet
// ─────────────────────────────────────────────────────────────────────────────

/// Analyse le contenu d'un bloc xattr (4096 octets typiquement).
///
/// # Safety
/// `data` doit pointer sur `size` octets initialisés et valides.
pub unsafe fn parse_xattr_block(data: *const u8, size: usize) -> FsResult<Vec<XattrEntry>> {
    // Vérifie le magic (u32 LE au début du bloc)
    let magic = (data as *const u32).read_unaligned().to_le();
    if magic != EXT4_XATTR_MAGIC { return Err(FsError::Corrupt); }

    let mut entries = Vec::new();
    let mut ptr = data.add(4) as usize; // saute le magic
    let end = data as usize + size;

    while ptr + XATTR_ENTRY_HDR_SIZE <= end {
        let hdr = (ptr as *const Ext4XattrEntryHeader).read_unaligned();
        if hdr.e_name_len == 0 && hdr.e_name_index == 0 { break; } // entrée terminale

        let name_start = ptr + XATTR_ENTRY_HDR_SIZE;
        let name_end   = name_start + hdr.e_name_len as usize;
        if name_end > end { return Err(FsError::Corrupt); }

        let name: Vec<u8> = core::slice::from_raw_parts(name_start as *const u8, hdr.e_name_len as usize).to_vec();

        let val_start = data as usize + hdr.e_value_offs as usize;
        let val_end   = val_start + hdr.e_value_size as usize;
        if hdr.e_value_inum == 0 && val_end > end { return Err(FsError::Corrupt); }

        let value = if hdr.e_value_inum == 0 && hdr.e_value_size > 0 {
            core::slice::from_raw_parts(val_start as *const u8, hdr.e_value_size as usize).to_vec()
        } else { Vec::new() };

        entries.push(XattrEntry {
            namespace: XattrNamespace::from_u8(hdr.e_name_index),
            name,
            value,
            hash: hdr.e_hash,
        });
        // Avance de sizeof(header) + name_len arrondi à 4 octets
        let step = XATTR_ENTRY_HDR_SIZE + ((hdr.e_name_len as usize + 3) & !3);
        ptr += step;
    }
    XATTR_STATS.parses.fetch_add(1, Ordering::Relaxed);
    Ok(entries)
}

// ─────────────────────────────────────────────────────────────────────────────
// XattrCache — cache xattr par inode
// ─────────────────────────────────────────────────────────────────────────────

pub struct XattrCacheEntry {
    pub ino:     u64,
    pub entries: Vec<XattrEntry>,
}

pub struct XattrCache {
    cache: SpinLock<Vec<XattrCacheEntry>>,
}

impl XattrCache {
    pub const fn new() -> Self { Self { cache: SpinLock::new(Vec::new()) } }

    pub fn lookup(&self, ino: u64) -> Option<Vec<XattrEntry>> {
        let guard = self.cache.lock();
        guard.iter().find(|e| e.ino == ino).map(|e| e.entries.clone())
    }

    pub fn insert(&self, ino: u64, entries: Vec<XattrEntry>) {
        let mut guard = self.cache.lock();
        // On remplace si déjà présent
        if let Some(e) = guard.iter_mut().find(|e| e.ino == ino) {
            e.entries = entries;
        } else {
            guard.push(XattrCacheEntry { ino, entries });
        }
        XATTR_STATS.cache_inserts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn invalidate(&self, ino: u64) {
        let mut guard = self.cache.lock();
        guard.retain(|e| e.ino != ino);
        XATTR_STATS.invalidations.fetch_add(1, Ordering::Relaxed);
    }
}

pub static XATTR_CACHE: XattrCache = XattrCache::new();

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Récupère un xattr par namespace + nom.
pub fn xattr_get(
    entries: &[XattrEntry],
    ns:      XattrNamespace,
    name:    &[u8],
) -> Option<Vec<u8>> {
    entries.iter()
        .find(|e| e.namespace == ns && e.name == name)
        .map(|e| e.value.clone())
}

/// Définit / remplace un xattr dans un vecteur en mémoire.
pub fn xattr_set(
    entries: &mut Vec<XattrEntry>,
    ns:      XattrNamespace,
    name:    Vec<u8>,
    value:   Vec<u8>,
) {
    if let Some(e) = entries.iter_mut().find(|e| e.namespace == ns && e.name == name) {
        e.value = value;
        e.hash  = e.compute_hash();
    } else {
        let mut entry = XattrEntry { namespace: ns, name, value, hash: 0 };
        entry.hash = entry.compute_hash();
        entries.push(entry);
    }
    XATTR_STATS.sets.fetch_add(1, Ordering::Relaxed);
}

/// Supprime un xattr.
pub fn xattr_remove(entries: &mut Vec<XattrEntry>, ns: XattrNamespace, name: &[u8]) -> bool {
    let before = entries.len();
    entries.retain(|e| !(e.namespace == ns && e.name == name));
    let removed = before != entries.len();
    if removed { XATTR_STATS.removes.fetch_add(1, Ordering::Relaxed); }
    removed
}

// ─────────────────────────────────────────────────────────────────────────────
// XattrStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct XattrStats {
    pub parses:        AtomicU64,
    pub cache_inserts: AtomicU64,
    pub invalidations: AtomicU64,
    pub sets:          AtomicU64,
    pub removes:       AtomicU64,
    pub errors:        AtomicU64,
}

impl XattrStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { parses: z!(), cache_inserts: z!(), invalidations: z!(), sets: z!(), removes: z!(), errors: z!() }
    }
}

pub static XATTR_STATS: XattrStats = XattrStats::new();
