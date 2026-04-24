//! validation.rs — Helpers userspace↔kernel pour syscalls ExoFS (no_std).
//!
//! RÈGLE 9  : copy_from_user() pour TOUT pointeur userspace entrant.
//! RÈGLE 10 : buffers PATH_MAX alloués sur le tas (Vec), jamais sur la stack.
//! RECUR-01 : zéro boucle `for`, uniquement `while`.
//! OOM-02   : try_reserve() avant tout push().
//! ARITH-02 : saturating_*/checked_div/wrapping_* uniquement.

#![allow(clippy::let_and_return)]

use crate::fs::exofs::core::rights::{
    ALL_RIGHTS, RIGHT_ADMIN, RIGHT_CREATE, RIGHT_DELETE, RIGHT_EXPORT, RIGHT_GC_TRIGGER,
    RIGHT_IMPORT, RIGHT_INSPECT_CONTENT, RIGHT_LIST, RIGHT_READ, RIGHT_RELATION_CREATE,
    RIGHT_SETMETA, RIGHT_SNAPSHOT_CREATE, RIGHT_STAT, RIGHT_WRITE,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::mem;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de bornage
// ─────────────────────────────────────────────────────────────────────────────

pub const EXOFS_PATH_MAX: usize = 4_096;
pub const EXOFS_NAME_MAX: usize = 255;
pub const EXOFS_BLOB_MAX: usize = 16 * 1_024 * 1_024;
pub const EXOFS_META_MAX: usize = 1_024;
pub const EXOFS_LIST_MAX: usize = 256 * 32;
pub const EXOFS_SNAP_LIST_MAX: usize = 128;
pub const EXOFS_SNAP_NAME_MAX: usize = 64;
pub const EXOFS_FD_INVALID: u32 = u32::MAX;
pub const EXOFS_FD_MIN: u32 = 4;
pub const EXOFS_FD_MAX: u32 = 65_535;

// ─────────────────────────────────────────────────────────────────────────────
// Codes errno POSIX
// ─────────────────────────────────────────────────────────────────────────────

pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const EIO: i64 = -5;
pub const EBADF: i64 = -9;
pub const ENOMEM: i64 = -12;
pub const EACCES: i64 = -13;
pub const EFAULT: i64 = -14;
pub const EBUSY: i64 = -16;
pub const EEXIST: i64 = -17;
pub const ENODIR: i64 = -20;
pub const EINVAL: i64 = -22;
pub const ENOSPC: i64 = -28;
pub const ENOSYS: i64 = -38;
pub const ERANGE: i64 = -34;
pub const EPROTO: i64 = -71;
pub const EBADMSG: i64 = -74;
pub const EOVERFLOW: i64 = -75;
pub const ENOTSUP: i64 = -95;
pub const EKEYREV: i64 = -126;
pub const ENOEPOCH: i64 = -130;
pub const EGCFULL: i64 = -131;
pub const EQUOTA: i64 = -132;
pub const EEPOCHFULL: i64 = -133;
pub const ECOMMIT: i64 = -134;

// ─────────────────────────────────────────────────────────────────────────────
// ExofsError → errno POSIX
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
pub fn exofs_err_to_errno(e: ExofsError) -> i64 {
    match e {
        ExofsError::NoMemory => ENOMEM,
        ExofsError::NoSpace => ENOSPC,
        ExofsError::IoError => EIO,
        ExofsError::PartialWrite => EIO,
        ExofsError::OffsetOverflow => EOVERFLOW,
        ExofsError::InvalidMagic => EBADMSG,
        ExofsError::ChecksumMismatch => EBADMSG,
        ExofsError::IncompatibleVersion => EPROTO,
        ExofsError::CorruptedStructure => EBADMSG,
        ExofsError::CorruptedChain => EBADMSG,
        ExofsError::ObjectNotFound => ENOENT,
        ExofsError::BlobNotFound => ENOENT,
        ExofsError::ObjectAlreadyExists => EEXIST,
        ExofsError::WrongObjectKind => EINVAL,
        ExofsError::WrongObjectClass => EINVAL,
        ExofsError::InvalidPathComponent => EINVAL,
        ExofsError::PathTooLong => ERANGE,
        ExofsError::TooManySymlinks => ERANGE,
        ExofsError::DirectoryNotEmpty => EBUSY,
        ExofsError::NotADirectory => ENODIR,
        ExofsError::PermissionDenied => EACCES,
        ExofsError::QuotaExceeded => EQUOTA,
        ExofsError::SecretBlobIdLeakPrevented => EACCES,
        ExofsError::NoValidEpoch => ENOEPOCH,
        ExofsError::EpochFull => EEPOCHFULL,
        ExofsError::CommitInProgress => ECOMMIT,
        ExofsError::GcQueueFull => EGCFULL,
        ExofsError::RefCountUnderflow => EIO,
        ExofsError::NotSupported => ENOTSUP,
        ExofsError::InvalidArgument => EINVAL,
        ExofsError::InternalError => EIO,
        _ => EIO,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Capability checks (Phase 2)
// ─────────────────────────────────────────────────────────────────────────────

/// Type de capability attendu par un handler ExoFS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityType {
    ExoFsPathResolve,
    ExoFsOpenRead,
    ExoFsOpenWrite,
    ExoFsObjectRead,
    ExoFsObjectWrite,
    ExoFsObjectCreate,
    ExoFsObjectDelete,
    ExoFsObjectStat,
    ExoFsObjectSetMeta,
    ExoFsGetContentHash,
    ExoFsSnapshotCreate,
    ExoFsSnapshotList,
    ExoFsSnapshotMount,
    ExoFsRelationCreate,
    ExoFsRelationQuery,
    ExoFsGcTrigger,
    ExoFsQuotaQuery,
    ExoFsQuotaSet,
    ExoFsExportObject,
    ExoFsImportObject,
    ExoFsEpochCommit,
    ExoFsOpenByPathRead,
    ExoFsOpenByPathWrite,
    ExoFsReaddir,
}

#[inline]
const fn required_right_for(cap: CapabilityType) -> u32 {
    match cap {
        CapabilityType::ExoFsPathResolve => RIGHT_READ,
        CapabilityType::ExoFsOpenRead => RIGHT_READ,
        CapabilityType::ExoFsOpenWrite => RIGHT_WRITE,
        CapabilityType::ExoFsObjectRead => RIGHT_READ,
        CapabilityType::ExoFsObjectWrite => RIGHT_WRITE,
        CapabilityType::ExoFsObjectCreate => RIGHT_CREATE,
        CapabilityType::ExoFsObjectDelete => RIGHT_DELETE,
        CapabilityType::ExoFsObjectStat => RIGHT_STAT,
        CapabilityType::ExoFsObjectSetMeta => RIGHT_SETMETA,
        CapabilityType::ExoFsGetContentHash => RIGHT_INSPECT_CONTENT,
        CapabilityType::ExoFsSnapshotCreate => RIGHT_SNAPSHOT_CREATE,
        CapabilityType::ExoFsSnapshotList => RIGHT_READ,
        CapabilityType::ExoFsSnapshotMount => RIGHT_READ,
        CapabilityType::ExoFsRelationCreate => RIGHT_RELATION_CREATE,
        CapabilityType::ExoFsRelationQuery => RIGHT_READ,
        CapabilityType::ExoFsGcTrigger => RIGHT_GC_TRIGGER,
        CapabilityType::ExoFsQuotaQuery => RIGHT_READ,
        CapabilityType::ExoFsQuotaSet => RIGHT_ADMIN,
        CapabilityType::ExoFsExportObject => RIGHT_EXPORT,
        CapabilityType::ExoFsImportObject => RIGHT_IMPORT,
        CapabilityType::ExoFsEpochCommit => RIGHT_WRITE,
        CapabilityType::ExoFsOpenByPathRead => RIGHT_READ,
        CapabilityType::ExoFsOpenByPathWrite => RIGHT_WRITE,
        CapabilityType::ExoFsReaddir => RIGHT_LIST,
    }
}

/// Vérifie les droits capability ExoFS.
///
/// `cap_rights_raw` est interprété comme un bitmask de droits ExoFS (u32).
/// Retourne `EPERM` si le droit requis est absent.
#[inline]
pub fn verify_cap(cap_rights_raw: u64, cap: CapabilityType) -> Result<(), i64> {
    let effective = (cap_rights_raw as u32) & ALL_RIGHTS;
    let required = required_right_for(cap);
    if effective & required == required {
        Ok(())
    } else {
        Err(EPERM)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// copy_from_user / copy_to_user
// ─────────────────────────────────────────────────────────────────────────────

/// Copie `len` octets depuis userspace (`src`) vers le buffer noyau `dst`.
///
/// # Safety
/// `dst` doit être un buffer noyau valide ≥ `len` octets.
/// `src` est une adresse userspace non nulle (vérifiée par l'appelant).
#[inline]
pub unsafe fn copy_from_user(dst: *mut u8, src: *const u8, len: usize) -> ExofsResult<()> {
    if src.is_null() || dst.is_null() {
        return Err(ExofsError::InvalidArgument);
    }
    if len == 0 {
        return Ok(());
    }
    core::ptr::copy_nonoverlapping(src, dst, len);
    Ok(())
}

/// Copie `len` octets depuis le buffer noyau `src` vers userspace `dst`.
///
/// # Safety
/// `src` doit être un buffer noyau valide ≥ `len` octets.
/// `dst` est une adresse userspace non nulle (vérifiée par l'appelant).
#[inline]
pub unsafe fn copy_to_user(dst: *mut u8, src: *const u8, len: usize) -> ExofsResult<()> {
    if dst.is_null() || src.is_null() {
        return Err(ExofsError::InvalidArgument);
    }
    if len == 0 {
        return Ok(());
    }
    core::ptr::copy_nonoverlapping(src, dst, len);
    Ok(())
}

/// Copie une structure `T` depuis userspace.
///
/// # Safety
/// `ptr` doit être aligné sur `align_of::<T>()` et pointer vers au moins
/// `size_of::<T>()` octets valides en userspace.
#[inline]
pub unsafe fn copy_struct_from_user<T: Copy>(ptr: u64) -> ExofsResult<T> {
    if ptr == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    if (ptr as usize) % mem::align_of::<T>() != 0 {
        return Err(ExofsError::InvalidArgument);
    }
    let mut val = mem::MaybeUninit::<T>::uninit();
    copy_from_user(
        val.as_mut_ptr() as *mut u8,
        ptr as *const u8,
        mem::size_of::<T>(),
    )?;
    Ok(val.assume_init())
}

/// Écrit une structure `T` vers userspace.
///
/// # Safety
/// `ptr` doit pointer vers un buffer userspace aligné.
#[inline]
pub unsafe fn copy_struct_to_user<T: Copy>(ptr: u64, val: &T) -> ExofsResult<()> {
    if ptr == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    if (ptr as usize) % mem::align_of::<T>() != 0 {
        return Err(ExofsError::InvalidArgument);
    }
    copy_to_user(
        ptr as *mut u8,
        val as *const T as *const u8,
        mem::size_of::<T>(),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers heap-allocated (RÈGLE 10)
// ─────────────────────────────────────────────────────────────────────────────

/// Lit un chemin C-string depuis userspace dans un `Vec` alloué sur le tas.
/// Retourne la longueur (sans NUL).
pub fn read_user_path_heap(ptr: u64, out: &mut Vec<u8>) -> Result<usize, i64> {
    if ptr == 0 {
        return Err(EFAULT);
    }
    out.clear();
    out.try_reserve(EXOFS_PATH_MAX).map_err(|_| ENOMEM)?;
    out.resize(EXOFS_PATH_MAX, 0u8);
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        copy_from_user(out.as_mut_ptr(), ptr as *const u8, EXOFS_PATH_MAX).map_err(|_| EFAULT)?;
    }
    // RECUR-01 : while.
    let mut i = 0usize;
    while i < EXOFS_PATH_MAX {
        if out[i] == 0 {
            break;
        }
        i = i.wrapping_add(1);
    }
    if i == 0 {
        return Err(EINVAL);
    }
    if i >= EXOFS_PATH_MAX {
        return Err(ERANGE);
    }
    Ok(i)
}

/// Lit un nom court (≤ `max`, ≤ EXOFS_NAME_MAX) depuis userspace.
pub fn read_user_name_heap(ptr: u64, max: usize, out: &mut Vec<u8>) -> Result<usize, i64> {
    if ptr == 0 {
        return Err(EFAULT);
    }
    let cap = max.min(EXOFS_NAME_MAX).saturating_add(1);
    out.clear();
    out.try_reserve(cap).map_err(|_| ENOMEM)?;
    out.resize(cap, 0u8);
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        copy_from_user(out.as_mut_ptr(), ptr as *const u8, cap).map_err(|_| EFAULT)?;
    }
    let mut i = 0usize;
    while i < cap {
        if out[i] == 0 {
            break;
        }
        i = i.wrapping_add(1);
    }
    if i == 0 {
        return Err(EINVAL);
    }
    Ok(i)
}

/// Lit un buffer binaire borné depuis userspace.
pub fn read_user_buf(ptr: u64, len: u64, out: &mut Vec<u8>) -> Result<(), i64> {
    let len = len as usize;
    if ptr == 0 {
        return Err(EFAULT);
    }
    if len == 0 {
        return Err(EINVAL);
    }
    if len > EXOFS_BLOB_MAX {
        return Err(ERANGE);
    }
    out.clear();
    out.try_reserve(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0u8);
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        copy_from_user(out.as_mut_ptr(), ptr as *const u8, len).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Écrit un buffer noyau vers userspace.
pub fn write_user_buf(dst: u64, src: &[u8]) -> Result<(), i64> {
    if dst == 0 {
        return Err(EFAULT);
    }
    if src.is_empty() {
        return Ok(());
    }
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    unsafe {
        copy_to_user(dst as *mut u8, src.as_ptr(), src.len()).map_err(|_| EFAULT)?;
    }
    Ok(())
}

/// Écrit un u64 optionnel vers userspace (ignore si pointeur nul).
#[inline]
pub fn write_user_u64_opt(dst: u64, val: u64) -> Result<(), i64> {
    if dst == 0 {
        return Ok(());
    }
    write_user_buf(dst, &val.to_le_bytes())
}

/// Écrit un u32 optionnel vers userspace.
#[inline]
pub fn write_user_u32_opt(dst: u64, val: u32) -> Result<(), i64> {
    if dst == 0 {
        return Ok(());
    }
    write_user_buf(dst, &val.to_le_bytes())
}

/// Écrit un i64 optionnel vers userspace.
#[inline]
pub fn write_user_i64_opt(dst: u64, val: i64) -> Result<(), i64> {
    if dst == 0 {
        return Ok(());
    }
    write_user_buf(dst, &val.to_le_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
// Validateurs d'arguments syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Valide un numéro de fd ExoFS (4 ≤ fd ≤ 65535).
#[inline]
pub fn validate_fd(fd: u64) -> Result<u32, i64> {
    if fd < EXOFS_FD_MIN as u64 || fd > EXOFS_FD_MAX as u64 {
        return Err(EBADF);
    }
    Ok(fd as u32)
}

/// Valide un count de transfert (1 ≤ count ≤ EXOFS_BLOB_MAX).
#[inline]
pub fn validate_count(count: u64) -> Result<usize, i64> {
    if count == 0 {
        return Err(EINVAL);
    }
    if count > EXOFS_BLOB_MAX as u64 {
        return Err(ERANGE);
    }
    Ok(count as usize)
}

/// Valide un offset disque (≤ 2^48 − 1).
#[inline]
pub fn validate_offset(offset: u64) -> Result<u64, i64> {
    const MAX_OFFSET: u64 = (1u64 << 48).wrapping_sub(1);
    if offset > MAX_OFFSET {
        return Err(EOVERFLOW);
    }
    Ok(offset)
}

/// Valide des flags d'ouverture ExoFS.
#[inline]
pub fn validate_open_flags(flags: u64) -> Result<u32, i64> {
    const VALID_FLAGS: u32 = 0x0000_07FF;
    let f = flags as u32;
    if f & !VALID_FLAGS != 0 {
        return Err(EINVAL);
    }
    Ok(f)
}

/// Valide un pointeur userspace non nul et aligné sur `align`.
#[inline]
pub fn validate_user_ptr(ptr: u64, align: usize) -> Result<(), i64> {
    if ptr == 0 {
        return Err(EFAULT);
    }
    if align > 1 && (ptr as usize) % align != 0 {
        return Err(EINVAL);
    }
    Ok(())
}

/// Valide qu'un identifiant 32 octets est ni tout-FF ni tout-zéro.
#[inline]
pub fn validate_id32(bytes: &[u8; 32]) -> Result<(), i64> {
    let mut all_ff = true;
    let mut all_zero = true;
    let mut i = 0usize;
    while i < 32 {
        if bytes[i] != 0xFF {
            all_ff = false;
        }
        if bytes[i] != 0x00 {
            all_zero = false;
        }
        i = i.wrapping_add(1);
    }
    if all_ff || all_zero {
        return Err(EINVAL);
    }
    Ok(())
}

/// Valide une longueur de métadonnée (1 ≤ len ≤ EXOFS_META_MAX).
#[inline]
pub fn validate_meta_len(len: u64) -> Result<usize, i64> {
    if len == 0 {
        return Err(EINVAL);
    }
    if len > EXOFS_META_MAX as u64 {
        return Err(ERANGE);
    }
    Ok(len as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// UserPtr<T>
// ─────────────────────────────────────────────────────────────────────────────

/// Wrapper sémantique pour un pointeur userspace typé.
#[derive(Copy, Clone, Debug)]
pub struct UserPtr<T> {
    addr: u64,
    _phantom: core::marker::PhantomData<*mut T>,
}

impl<T: Copy> UserPtr<T> {
    #[inline]
    pub fn new(addr: u64) -> Self {
        Self {
            addr,
            _phantom: core::marker::PhantomData,
        }
    }

    #[inline]
    pub fn addr(self) -> u64 {
        self.addr
    }

    #[inline]
    pub fn is_null(self) -> bool {
        self.addr == 0
    }

    /// # Safety — adresse userspace valide de taille `size_of::<T>()`.
    pub unsafe fn read(self) -> ExofsResult<T> {
        copy_struct_from_user::<T>(self.addr)
    }

    /// # Safety — adresse userspace valide de taille `size_of::<T>()`.
    pub unsafe fn write(self, val: &T) -> ExofsResult<()> {
        copy_struct_to_user::<T>(self.addr, val)
    }
}

unsafe impl<T> Send for UserPtr<T> {}
unsafe impl<T> Sync for UserPtr<T> {}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers retour syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la valeur syscall depuis un Result<i64, i64>.
#[inline]
pub fn syscall_ret(r: Result<i64, i64>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// Convertit un ExofsResult<i64> en code de retour syscall.
#[inline]
pub fn exofs_ret(r: ExofsResult<i64>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => exofs_err_to_errno(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_errno_nomem() {
        assert_eq!(exofs_err_to_errno(ExofsError::NoMemory), ENOMEM);
    }

    #[test]
    fn test_errno_nospace() {
        assert_eq!(exofs_err_to_errno(ExofsError::NoSpace), ENOSPC);
    }

    #[test]
    fn test_errno_notfound() {
        assert_eq!(exofs_err_to_errno(ExofsError::ObjectNotFound), ENOENT);
        assert_eq!(exofs_err_to_errno(ExofsError::BlobNotFound), ENOENT);
    }

    #[test]
    fn test_errno_perm() {
        assert_eq!(exofs_err_to_errno(ExofsError::PermissionDenied), EACCES);
    }

    #[test]
    fn test_errno_quota() {
        assert_eq!(exofs_err_to_errno(ExofsError::QuotaExceeded), EQUOTA);
    }

    #[test]
    fn test_errno_epoch() {
        assert_eq!(exofs_err_to_errno(ExofsError::NoValidEpoch), ENOEPOCH);
        assert_eq!(exofs_err_to_errno(ExofsError::EpochFull), EEPOCHFULL);
        assert_eq!(exofs_err_to_errno(ExofsError::CommitInProgress), ECOMMIT);
    }

    #[test]
    fn test_validate_fd_ok() {
        assert_eq!(validate_fd(4).unwrap(), 4u32);
        assert_eq!(validate_fd(65535).unwrap(), 65535u32);
    }

    #[test]
    fn test_validate_fd_low() {
        assert_eq!(validate_fd(0).unwrap_err(), EBADF);
        assert_eq!(validate_fd(3).unwrap_err(), EBADF);
    }

    #[test]
    fn test_validate_fd_high() {
        assert_eq!(validate_fd(65536).unwrap_err(), EBADF);
    }

    #[test]
    fn test_validate_count_ok() {
        assert_eq!(validate_count(1).unwrap(), 1);
        assert_eq!(validate_count(4096).unwrap(), 4096);
    }

    #[test]
    fn test_validate_count_zero() {
        assert_eq!(validate_count(0).unwrap_err(), EINVAL);
    }

    #[test]
    fn test_validate_count_overflow() {
        assert_eq!(
            validate_count(EXOFS_BLOB_MAX as u64 + 1).unwrap_err(),
            ERANGE
        );
    }

    #[test]
    fn test_validate_offset_ok() {
        assert!(validate_offset(0).is_ok());
        assert!(validate_offset(1u64 << 40).is_ok());
    }

    #[test]
    fn test_validate_offset_overflow() {
        assert_eq!(validate_offset(u64::MAX).unwrap_err(), EOVERFLOW);
    }

    #[test]
    fn test_validate_flags_ok() {
        assert!(validate_open_flags(0x0001).is_ok());
        assert!(validate_open_flags(0x0002).is_ok());
    }

    #[test]
    fn test_validate_flags_bad() {
        assert_eq!(validate_open_flags(0x8000_0000).unwrap_err(), EINVAL);
    }

    #[test]
    fn test_validate_id32_invalid() {
        let ff = [0xFFu8; 32];
        let zz = [0x00u8; 32];
        assert_eq!(validate_id32(&ff).unwrap_err(), EINVAL);
        assert_eq!(validate_id32(&zz).unwrap_err(), EINVAL);
    }

    #[test]
    fn test_validate_id32_valid() {
        let mut id = [0xFFu8; 32];
        id[0] = 0x01;
        assert!(validate_id32(&id).is_ok());
    }

    #[test]
    fn test_write_user_u64_opt_null() {
        assert!(write_user_u64_opt(0, 42).is_ok());
    }

    #[test]
    fn test_write_user_u32_opt_null() {
        assert!(write_user_u32_opt(0, 77).is_ok());
    }

    #[test]
    fn test_validate_user_ptr_null() {
        assert_eq!(validate_user_ptr(0, 1).unwrap_err(), EFAULT);
    }

    #[test]
    fn test_validate_user_ptr_misaligned() {
        assert_eq!(validate_user_ptr(3, 8).unwrap_err(), EINVAL);
    }

    #[test]
    fn test_validate_user_ptr_ok() {
        assert!(validate_user_ptr(0x1000, 8).is_ok());
    }

    #[test]
    fn test_syscall_ret() {
        assert_eq!(syscall_ret(Ok(42)), 42);
        assert_eq!(syscall_ret(Err(EINVAL)), EINVAL);
    }

    #[test]
    fn test_exofs_ret() {
        assert_eq!(exofs_ret(Ok(0)), 0);
        assert_eq!(exofs_ret(Err(ExofsError::NoMemory)), ENOMEM);
    }

    #[test]
    fn test_userptr_null() {
        let p: UserPtr<u64> = UserPtr::new(0);
        assert!(p.is_null());
    }

    #[test]
    fn test_userptr_not_null() {
        let p: UserPtr<u64> = UserPtr::new(0x8000);
        assert!(!p.is_null());
        assert_eq!(p.addr(), 0x8000);
    }

    #[test]
    fn test_validate_meta_len_ok() {
        assert_eq!(validate_meta_len(1).unwrap(), 1);
        assert_eq!(
            validate_meta_len(EXOFS_META_MAX as u64).unwrap(),
            EXOFS_META_MAX
        );
    }

    #[test]
    fn test_validate_meta_len_bad() {
        assert_eq!(validate_meta_len(0).unwrap_err(), EINVAL);
        assert_eq!(
            validate_meta_len(EXOFS_META_MAX as u64 + 1).unwrap_err(),
            ERANGE
        );
    }

    #[test]
    fn test_verify_cap_ok() {
        assert!(verify_cap(RIGHT_READ as u64, CapabilityType::ExoFsObjectRead).is_ok());
    }

    #[test]
    fn test_verify_cap_denied() {
        assert_eq!(
            verify_cap(0, CapabilityType::ExoFsObjectRead).unwrap_err(),
            EPERM
        );
    }
}
