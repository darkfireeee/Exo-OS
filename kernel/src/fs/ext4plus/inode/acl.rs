// kernel/src/fs/ext4plus/inode/acl.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ POSIX ACL — access control lists (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente les ACL POSIX.1e (access et default) stockées en xattr :
//   system.posix_acl_access  (XattrNamespace::PosixAclAccess)
//   system.posix_acl_default (XattrNamespace::PosixAclDefault)
//
// Format binaire des ACL (Linux acl.h) :
//   header  : acl_ea_header (4 octets) — magic + version
//   entries : N × acl_ea_entry  (8 octets chacune)
//
// Types d'entrée :
//   ACL_USER_OBJ   0x01   → permissions du propriétaire
//   ACL_USER       0x02   → entrée pour un UID spécifique
//   ACL_GROUP_OBJ  0x04   → permissions du groupe propriétaire
//   ACL_GROUP      0x08   → entrée pour un GID spécifique
//   ACL_MASK       0x10   → masque maximal
//   ACL_OTHER      0x20   → permissions pour tous les autres
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

use crate::fs::core::types::{FsError, FsResult, FileMode};
use crate::fs::ext4plus::inode::xattr::{XattrEntry, XattrNamespace, xattr_get};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const ACL_EA_MAGIC:   u32 = 0x0002_0001;
pub const ACL_EA_VERSION: u32 = 2;

pub const ACL_USER_OBJ:  u16 = 0x01;
pub const ACL_USER:      u16 = 0x02;
pub const ACL_GROUP_OBJ: u16 = 0x04;
pub const ACL_GROUP:     u16 = 0x08;
pub const ACL_MASK:      u16 = 0x10;
pub const ACL_OTHER:     u16 = 0x20;

pub const ACL_PERM_READ:    u16 = 0x04;
pub const ACL_PERM_WRITE:   u16 = 0x02;
pub const ACL_PERM_EXECUTE: u16 = 0x01;

// ─────────────────────────────────────────────────────────────────────────────
// Structures on-disk
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct AclEaHeader {
    pub a_version: u32,  // 2
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct AclEaEntry {
    pub e_tag:  u16,
    pub e_perm: u16,
    pub e_id:   u32,  // UID ou GID (seulement pour ACL_USER/ACL_GROUP)
}

// ─────────────────────────────────────────────────────────────────────────────
// AclEntry — représentation en mémoire
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AclEntry {
    pub tag:  u16,
    pub perm: u16,
    pub id:   Option<u32>,  // uid/gid si tag == ACL_USER ou ACL_GROUP
}

impl AclEntry {
    pub fn matches_uid(&self, uid: u32) -> bool {
        self.tag == ACL_USER && self.id == Some(uid)
    }
    pub fn matches_gid(&self, gid: u32) -> bool {
        self.tag == ACL_GROUP && self.id == Some(gid)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// parse_acl — désérialise un blob xattr en Vec<AclEntry>
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_acl(data: &[u8]) -> FsResult<Vec<AclEntry>> {
    if data.len() < 4 { return Err(FsError::Corrupt); }
    // SAFETY: data.len() >= 4, lecture d'un u32 little-endian
    let version = unsafe { (data.as_ptr() as *const u32).read_unaligned() }.to_le();
    if version != ACL_EA_VERSION { return Err(FsError::Corrupt); }

    let entry_size = core::mem::size_of::<AclEaEntry>();
    let body = &data[4..];
    if body.len() % entry_size != 0 { return Err(FsError::Corrupt); }

    let count = body.len() / entry_size;
    let mut entries = Vec::with_capacity(count);

    for i in 0..count {
        // SAFETY: body indexé correctement à entry_size * i
        let raw = unsafe {
            (body.as_ptr().add(i * entry_size) as *const AclEaEntry).read_unaligned()
        };
        let id = match raw.e_tag {
            ACL_USER | ACL_GROUP => Some(raw.e_id),
            _ => None,
        };
        entries.push(AclEntry { tag: raw.e_tag, perm: raw.e_perm, id });
    }
    ACL_STATS.parses.fetch_add(1, Ordering::Relaxed);
    Ok(entries)
}

// ─────────────────────────────────────────────────────────────────────────────
// acl_check — vérifie si uid/gid a le droit `perm` (READ|WRITE|EXECUTE)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie les permissions PAX pour `uid` / `gid` demandant `requested_perm`.
/// Applique l'algorithme POSIX.1e :
///   1. ACL_USER_OBJ → si uid == inode.uid
///   2. ACL_USER     → si correspondance uid
///   3. ACL_GROUP_OBJ / ACL_GROUP → si correspondance gid
///   4. ACL_OTHER sinon
/// La permission finale est ANDée avec ACL_MASK si présent.
pub fn acl_check(
    acl:   &[AclEntry],
    uid:   u32,
    gid:   u32,
    inode_uid: u32,
    inode_gid: u32,
    requested: u16,
) -> bool {
    // Récupère le masque (s'il existe)
    let mask = acl.iter()
        .find(|e| e.tag == ACL_MASK)
        .map(|e| e.perm)
        .unwrap_or(0xFFFF);

    // Étape 1 : propriétaire
    if uid == inode_uid {
        let eff = acl.iter().find(|e| e.tag == ACL_USER_OBJ).map(|e| e.perm).unwrap_or(0);
        return (eff & requested) == requested;
    }

    // Étape 2 : ACL_USER spécifique
    if let Some(ue) = acl.iter().find(|e| e.matches_uid(uid)) {
        let eff = ue.perm & mask;
        return (eff & requested) == requested;
    }

    // Étape 3 : groupe propriétaire ou ACL_GROUP spécifique
    let in_owner_group   = gid == inode_gid;
    let group_obj_match = acl.iter().find(|e| e.tag == ACL_GROUP_OBJ);
    let group_matched   = acl.iter().find(|e| e.matches_gid(gid));

    if in_owner_group || group_matched.is_some() {
        let perm = group_matched
            .map(|e| e.perm)
            .or_else(|| group_obj_match.map(|e| e.perm))
            .unwrap_or(0);
        let eff = perm & mask;
        return (eff & requested) == requested;
    }

    // Étape 4 : autres
    let other = acl.iter().find(|e| e.tag == ACL_OTHER).map(|e| e.perm).unwrap_or(0);
    (other & requested) == requested
}

// ─────────────────────────────────────────────────────────────────────────────
// acl_from_xattrs — construit les ACL depuis la liste d'attributs étendus
// ─────────────────────────────────────────────────────────────────────────────

pub fn acl_access_from_xattrs(xattrs: &[XattrEntry]) -> FsResult<Vec<AclEntry>> {
    let data = xattr_get(xattrs, XattrNamespace::PosixAclAccess, b"")
        .ok_or(FsError::NotFound)?;
    parse_acl(&data)
}

pub fn acl_default_from_xattrs(xattrs: &[XattrEntry]) -> FsResult<Vec<AclEntry>> {
    let data = xattr_get(xattrs, XattrNamespace::PosixAclDefault, b"")
        .ok_or(FsError::NotFound)?;
    parse_acl(&data)
}

// ─────────────────────────────────────────────────────────────────────────────
// mode_to_acl / acl_to_mode — conversion mode Unix ↔ ACL minimaliste
// ─────────────────────────────────────────────────────────────────────────────

/// Construit une ACL minimale à 3 entrées depuis un mode Unix (rwxrwxrwx).
pub fn mode_to_acl(mode: u16) -> Vec<AclEntry> {
    let owner = ((mode >> 6) & 0x7) as u16;
    let group = ((mode >> 3) & 0x7) as u16;
    let other = (mode & 0x7) as u16;
    alloc::vec![
        AclEntry { tag: ACL_USER_OBJ,  perm: owner, id: None },
        AclEntry { tag: ACL_GROUP_OBJ, perm: group, id: None },
        AclEntry { tag: ACL_OTHER,     perm: other, id: None },
    ]
}

/// Reconstruit le mode Unix depuis une ACL (user_obj, group_obj/mask, other).
pub fn acl_to_mode(acl: &[AclEntry]) -> u16 {
    let owner = acl.iter().find(|e| e.tag == ACL_USER_OBJ).map(|e| e.perm).unwrap_or(0);
    let mask  = acl.iter().find(|e| e.tag == ACL_MASK).map(|e| e.perm)
                   .or_else(|| acl.iter().find(|e| e.tag == ACL_GROUP_OBJ).map(|e| e.perm))
                   .unwrap_or(0);
    let other = acl.iter().find(|e| e.tag == ACL_OTHER).map(|e| e.perm).unwrap_or(0);
    ((owner & 7) << 6) as u16 | ((mask & 7) << 3) as u16 | (other & 7) as u16
}

// ─────────────────────────────────────────────────────────────────────────────
// AclStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct AclStats {
    pub parses:   AtomicU64,
    pub checks:   AtomicU64,
    pub allowed:  AtomicU64,
    pub denied:   AtomicU64,
}

impl AclStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { parses: z!(), checks: z!(), allowed: z!(), denied: z!() }
    }
}

pub static ACL_STATS: AclStats = AclStats::new();
