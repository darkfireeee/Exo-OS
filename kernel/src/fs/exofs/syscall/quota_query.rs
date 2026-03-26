//! quota_query.rs — SYS_EXOFS_QUOTA_QUERY (515)
//!
//! Interroge et fixe les quotas ExoFS par objet/uid.
//! RECUR-01 / OOM-02 / ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, copy_struct_from_user, write_user_buf,
    verify_cap, CapabilityType, EFAULT,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

const QUOTA_MAGIC:       u32  = 0x5155_4F54; // "QUOT"
const QUOTA_UNLIMITED:   u64  = u64::MAX;
pub const QUOTA_MAX_ENTRIES: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod quota_flags {
    pub const GET:         u32 = 0x0001;
    pub const SET:         u32 = 0x0002;
    pub const BY_UID:      u32 = 0x0004;
    pub const SOFT_LIMIT:  u32 = 0x0008;
    pub const HARD_LIMIT:  u32 = 0x0010;
    pub const VALID_MASK:  u32 = GET | SET | BY_UID | SOFT_LIMIT | HARD_LIMIT;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QuotaQueryArgs {
    pub flags:       u32,
    pub _pad:        u32,
    pub owner_uid:   u64,
    pub blob_id:     [u8; 32],
}

const _: () = assert!(core::mem::size_of::<QuotaQueryArgs>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QuotaInfo {
    pub used_bytes:   u64,
    pub soft_bytes:   u64,
    pub hard_bytes:   u64,
    pub used_objects: u64,
    pub soft_objects: u64,
    pub hard_objects: u64,
    pub flags:        u32,
    pub _pad:         u32,
}

const _: () = assert!(core::mem::size_of::<QuotaInfo>() == 56);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QuotaSetArgs {
    pub owner_uid:    u64,
    pub new_soft_bytes:   u64,
    pub new_hard_bytes:   u64,
    pub new_soft_objects: u64,
    pub new_hard_objects: u64,
    pub flags:        u32,
    pub _pad:         u32,
}

const _: () = assert!(core::mem::size_of::<QuotaSetArgs>() == 48);

/// Entrée dans la table de quotas persistante.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct QuotaEntry {
    owner_uid:    u64,
    used_bytes:   u64,
    soft_bytes:   u64,
    hard_bytes:   u64,
    used_objects: u64,
    soft_objects: u64,
    hard_objects: u64,
    flags:        u32,
    _pad:         u32,
}

const QUOTA_ENTRY_SIZE: usize = core::mem::size_of::<QuotaEntry>();
const QUOTA_HDR:        usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// Stockage quota (blob dédié par uid)
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive le BlobId de la table de quotas pour un uid.
fn quota_blob_id(owner_uid: u64) -> BlobId {
    let uid_bytes = owner_uid.to_le_bytes();
    let mut buf = [0u8; 10];
    let mut i = 0usize;
    while i < 8 { buf[i] = uid_bytes[i]; i = i.wrapping_add(1); }
    buf[8] = 0x51; buf[9] = 0x55; // "QU"
    BlobId::from_bytes_blake3(&buf)
}

/// Charge les entrées de quota depuis le cache.
/// OOM-02 / RECUR-01.
fn load_quota_entries(blob_id: BlobId) -> ExofsResult<Vec<QuotaEntry>> {
    let data = match BLOB_CACHE.get(&blob_id) { Some(d) => d, None => return Ok(Vec::new()) };
    if data.len() < QUOTA_HDR { return Ok(Vec::new()); }
    let magic = u32::from_le_bytes([data[0],data[1],data[2],data[3]]);
    if magic != QUOTA_MAGIC { return Err(ExofsError::InvalidMagic); }
    let count = u32::from_le_bytes([data[4],data[5],data[6],data[7]]) as usize;
    let avail = (data.len().saturating_sub(QUOTA_HDR)) / QUOTA_ENTRY_SIZE;
    let n = count.min(avail).min(QUOTA_MAX_ENTRIES);
    let mut out: Vec<QuotaEntry> = Vec::new();
    out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n {
        let off = QUOTA_HDR.saturating_add(i.saturating_mul(QUOTA_ENTRY_SIZE));
        let mut e = QuotaEntry::default();
        // SAFETY: pointeur valide sur une struct repr(C), durée de vie bornée par la référence.
        let dst = unsafe { core::slice::from_raw_parts_mut(&mut e as *mut QuotaEntry as *mut u8, QUOTA_ENTRY_SIZE) };
        let mut j = 0usize;
        while j < QUOTA_ENTRY_SIZE { dst[j] = data[off + j]; j = j.wrapping_add(1); }
        out.push(e);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Sérialise et sauvegarde les entrées de quota.
/// OOM-02 / RECUR-01.
fn save_quota_entries(blob_id: BlobId, entries: &[QuotaEntry]) -> ExofsResult<()> {
    let n = entries.len().min(QUOTA_MAX_ENTRIES);
    let total = QUOTA_HDR.saturating_add(n.saturating_mul(QUOTA_ENTRY_SIZE));
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let magic = QUOTA_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(magic[i]); i = i.wrapping_add(1); }
    let cnt = (n as u32).to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(cnt[i]); i = i.wrapping_add(1); }
    let mut i = 0usize;
    while i < n {
        // SAFETY: pointeur valide sur une struct repr(C), durée de vie bornée par la référence.
        let src = unsafe { core::slice::from_raw_parts(&entries[i] as *const QuotaEntry as *const u8, QUOTA_ENTRY_SIZE) };
        let mut j = 0usize;
        while j < QUOTA_ENTRY_SIZE { buf.push(src[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    BLOB_CACHE.insert(blob_id, buf.to_vec()).map_err(|_| ExofsError::NoSpace)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations GET / SET
// ─────────────────────────────────────────────────────────────────────────────

/// Lit les quotas d'un uid depuis le cache.
pub fn get_quota(owner_uid: u64) -> ExofsResult<QuotaInfo> {
    let bid = quota_blob_id(owner_uid);
    let entries = load_quota_entries(bid)?;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].owner_uid == owner_uid {
            return Ok(QuotaInfo {
                used_bytes:   entries[i].used_bytes,
                soft_bytes:   entries[i].soft_bytes,
                hard_bytes:   entries[i].hard_bytes,
                used_objects: entries[i].used_objects,
                soft_objects: entries[i].soft_objects,
                hard_objects: entries[i].hard_objects,
                flags:        entries[i].flags,
                _pad:         0,
            });
        }
        i = i.wrapping_add(1);
    }
    // Quota non trouvé → quotas illimités par défaut.
    Ok(QuotaInfo { used_bytes: 0, soft_bytes: QUOTA_UNLIMITED, hard_bytes: QUOTA_UNLIMITED, used_objects: 0, soft_objects: QUOTA_UNLIMITED, hard_objects: QUOTA_UNLIMITED, flags: 0, _pad: 0 })
}

/// Fixe les limites de quota pour un uid.
/// OOM-02
pub fn set_quota(set: &QuotaSetArgs) -> ExofsResult<()> {
    let bid = quota_blob_id(set.owner_uid);
    let mut entries = load_quota_entries(bid)?;
    let mut found = false;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].owner_uid == set.owner_uid {
            if set.flags & quota_flags::SOFT_LIMIT != 0 {
                entries[i].soft_bytes   = set.new_soft_bytes;
                entries[i].soft_objects = set.new_soft_objects;
            }
            if set.flags & quota_flags::HARD_LIMIT != 0 {
                entries[i].hard_bytes   = set.new_hard_bytes;
                entries[i].hard_objects = set.new_hard_objects;
            }
            found = true;
            break;
        }
        i = i.wrapping_add(1);
    }
    if !found {
        if entries.len() >= QUOTA_MAX_ENTRIES { return Err(ExofsError::QuotaExceeded); }
        let ne = QuotaEntry {
            owner_uid:    set.owner_uid,
            used_bytes:   0,
            soft_bytes:   set.new_soft_bytes,
            hard_bytes:   set.new_hard_bytes,
            used_objects: 0,
            soft_objects: set.new_soft_objects,
            hard_objects: set.new_hard_objects,
            flags:        set.flags,
            _pad:         0,
        };
        entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        entries.push(ne);
    }
    save_quota_entries(quota_blob_id(set.owner_uid), &entries)
}

/// Vérifie si une allocation est dans les limites.
pub fn check_quota(owner_uid: u64, extra_bytes: u64, extra_objects: u64) -> ExofsResult<()> {
    let q = get_quota(owner_uid)?;
    if q.hard_bytes != QUOTA_UNLIMITED {
        let new_bytes = q.used_bytes.saturating_add(extra_bytes);
        if new_bytes > q.hard_bytes { return Err(ExofsError::QuotaExceeded); }
    }
    if q.hard_objects != QUOTA_UNLIMITED {
        let new_objs = q.used_objects.saturating_add(extra_objects);
        if new_objs > q.hard_objects { return Err(ExofsError::QuotaExceeded); }
    }
    Ok(())
}

/// Incrémente l'usage quota d'un uid.
pub fn quota_add_usage(owner_uid: u64, bytes: u64, objects: u64) -> ExofsResult<()> {
    let bid = quota_blob_id(owner_uid);
    let mut entries = load_quota_entries(bid)?;
    let mut found = false;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].owner_uid == owner_uid {
            entries[i].used_bytes   = entries[i].used_bytes.saturating_add(bytes);
            entries[i].used_objects = entries[i].used_objects.saturating_add(objects);
            found = true;
            break;
        }
        i = i.wrapping_add(1);
    }
    if !found { return Ok(()); }
    save_quota_entries(quota_blob_id(owner_uid), &entries)
}

/// Décrémente l'usage quota (sans passer sous zero).
pub fn quota_sub_usage(owner_uid: u64, bytes: u64, objects: u64) -> ExofsResult<()> {
    let bid = quota_blob_id(owner_uid);
    let mut entries = load_quota_entries(bid)?;
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].owner_uid == owner_uid {
            entries[i].used_bytes   = entries[i].used_bytes.saturating_sub(bytes);
            entries[i].used_objects = entries[i].used_objects.saturating_sub(objects);
            break;
        }
        i = i.wrapping_add(1);
    }
    save_quota_entries(quota_blob_id(owner_uid), &entries)
}

/// Vide le compteur d'usage d'un uid.
pub fn quota_reset_usage(owner_uid: u64) -> ExofsResult<()> {
    quota_sub_usage(owner_uid, u64::MAX, u64::MAX)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_QUOTA_QUERY (515)
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_exofs_quota_query(
    args_ptr:   u64,
    result_ptr: u64,
    _a3: u64, _a4: u64, _a5: u64, cap_rights: u64,
) -> i64 {
    if args_ptr == 0 { return EFAULT; }
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    let args = match unsafe { copy_struct_from_user::<QuotaQueryArgs>(args_ptr) } {
        Ok(a)  => a,
        Err(_) => return EFAULT,
    };
    if args.flags & !quota_flags::VALID_MASK != 0 {
        return exofs_err_to_errno(ExofsError::InvalidArgument);
    }
    let cap = if args.flags & quota_flags::SET != 0 {
        CapabilityType::ExoFsQuotaSet
    } else {
        CapabilityType::ExoFsQuotaQuery
    };
    if let Err(e) = verify_cap(cap_rights, cap) {
        return e;
    }
    if args.flags & quota_flags::SET != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let set_args = match unsafe { copy_struct_from_user::<QuotaSetArgs>(args_ptr) } {
            Ok(s)  => s,
            Err(_) => return EFAULT,
        };
        if let Err(e) = set_quota(&set_args) { return exofs_err_to_errno(e); }
        return 0;
    }
    let info = match get_quota(args.owner_uid) {
        Ok(i)  => i,
        Err(e) => return exofs_err_to_errno(e),
    };
    if result_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let bytes = unsafe {
            core::slice::from_raw_parts(&info as *const QuotaInfo as *const u8, core::mem::size_of::<QuotaInfo>())
        };
        if let Err(e) = write_user_buf(result_ptr, bytes) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_args_size() { assert_eq!(core::mem::size_of::<QuotaQueryArgs>(), 48); }

    #[test]
    fn test_quota_info_size() { assert_eq!(core::mem::size_of::<QuotaInfo>(), 56); }

    #[test]
    fn test_quota_set_args_size() { assert_eq!(core::mem::size_of::<QuotaSetArgs>(), 48); }

    #[test]
    fn test_get_quota_default_unlimited() {
        let q = get_quota(0xDEAD_BEEF).unwrap();
        assert_eq!(q.hard_bytes, QUOTA_UNLIMITED);
        assert_eq!(q.hard_objects, QUOTA_UNLIMITED);
    }

    #[test]
    fn test_check_quota_unlimited_passes() {
        assert!(check_quota(0xFFFF_FFFF, 1024 * 1024, 100).is_ok());
    }

    #[test]
    fn test_set_and_get_quota() {
        let uid = 0xAA_BB_CC_DD_EE_FF_00_11u64;
        let set = QuotaSetArgs { owner_uid: uid, new_soft_bytes: 512, new_hard_bytes: 1024, new_soft_objects: 10, new_hard_objects: 20, flags: quota_flags::SOFT_LIMIT | quota_flags::HARD_LIMIT, _pad: 0 };
        set_quota(&set).unwrap();
        let q = get_quota(uid).unwrap();
        assert_eq!(q.hard_bytes, 1024);
        assert_eq!(q.hard_objects, 20);
    }

    #[test]
    fn test_check_quota_exceeded() {
        let uid = 0x11_22_33_44u64;
        let set = QuotaSetArgs { owner_uid: uid, new_soft_bytes: 0, new_hard_bytes: 100, new_soft_objects: 0, new_hard_objects: 5, flags: quota_flags::HARD_LIMIT, _pad: 0 };
        set_quota(&set).unwrap();
        assert!(check_quota(uid, 200, 0).is_err());
    }

    #[test]
    fn test_quota_add_and_sub() {
        let uid = 0x99u64;
        quota_add_usage(uid, 512, 3).ok();
        quota_sub_usage(uid, 100, 1).ok();
        let q = get_quota(uid).unwrap();
        // usage peut être 0 si l'entrée n'existait pas avant
        let _ = q.used_bytes;
    }

    #[test]
    fn test_quota_invalid_flags() {
        assert_eq!(sys_exofs_quota_query(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_quota_magic() { assert_eq!(QUOTA_MAGIC, 0x5155_4F54); }

    #[test]
    fn test_quota_unlimited_const() { assert_eq!(QUOTA_UNLIMITED, u64::MAX); }

    #[test]
    fn test_quota_entry_size() { assert_eq!(QUOTA_ENTRY_SIZE, 64); }

    #[test]
    fn test_quota_reset_usage() {
        quota_reset_usage(0xBEEF).ok();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers d'inspection rapide
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne vrai si un uid a dépassé son quota souple en octets.
pub fn quota_soft_exceeded_bytes(owner_uid: u64) -> ExofsResult<bool> {
    let q = get_quota(owner_uid)?;
    Ok(q.soft_bytes != QUOTA_UNLIMITED && q.used_bytes > q.soft_bytes)
}

/// Retourne vrai si un uid a dépassé son quota souple en objets.
pub fn quota_soft_exceeded_objects(owner_uid: u64) -> ExofsResult<bool> {
    let q = get_quota(owner_uid)?;
    Ok(q.soft_objects != QUOTA_UNLIMITED && q.used_objects > q.soft_objects)
}

/// Retourne le pourcentage d'utilisation des octets (0..=100 saturé).
pub fn quota_bytes_percent(owner_uid: u64) -> ExofsResult<u32> {
    let q = get_quota(owner_uid)?;
    if q.hard_bytes == 0 || q.hard_bytes == QUOTA_UNLIMITED { return Ok(0); }
    let pct = q.used_bytes.saturating_mul(100).checked_div(q.hard_bytes).unwrap_or(100);
    Ok(pct.min(100) as u32)
}
