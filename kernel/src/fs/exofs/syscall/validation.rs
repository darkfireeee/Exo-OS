//! validation.rs — helpers copy_from_user, bornes, PATH_MAX pour syscalls ExoFS.
//! RÈGLE 9 : copy_from_user() pour tout pointeur userspace.
//! RÈGLE 10 : buffers PATH_MAX sur le tas uniquement.

use alloc::vec::Vec;
use crate::syscall::validation::{copy_from_user, copy_to_user, SyscallError};
use crate::fs::exofs::core::FsError;

pub const EXOFS_PATH_MAX: usize = 4096;
pub const EXOFS_NAME_MAX: usize = 255;
pub const EXOFS_BLOB_MAX:  usize = 16 * 1024 * 1024; // 16 MiB max par appel.

/// Errno ExoFS → valeur i64.
pub const ENOENT:  i64 = -2;
pub const ENOMEM:  i64 = -12;
pub const EFAULT:  i64 = -14;
pub const EEXIST:  i64 = -17;
pub const EINVAL:  i64 = -22;
pub const ENOSPC:  i64 = -28;
pub const ERANGE:  i64 = -34;
pub const ENOTSUP: i64 = -95;
pub const EBUSY:   i64 = -16;
pub const EOVERFLOW: i64 = -75;
pub const EBADMSG: i64 = -74;
pub const EKEYREV: i64 = -126; // Clé révoquée / intégrité.

pub fn fserr_to_errno(e: FsError) -> i64 {
    match e {
        FsError::NotFound            => ENOENT,
        FsError::OutOfMemory         => ENOMEM,
        FsError::InvalidArgument     => EINVAL,
        FsError::InvalidMagic        => EBADMSG,
        FsError::AuthTagMismatch     => EKEYREV,
        FsError::Overflow            => EOVERFLOW,
        FsError::Busy                => EBUSY,
        FsError::IntegrityCheckFailed=> EBADMSG,
        FsError::InvalidData         => EINVAL,
        _ => EINVAL,
    }
}

/// Lit un chemin depuis userspace (RÈGLE 9+10 : copie sur heap, max EXOFS_PATH_MAX).
pub fn read_user_path_heap(ptr: u64, out: &mut Vec<u8>) -> Result<usize, i64> {
    if ptr == 0 { return Err(EFAULT); }
    // Allouer un buffer heap (RÈGLE 10 : jamais sur stack).
    out.clear();
    out.try_reserve(EXOFS_PATH_MAX).map_err(|_| ENOMEM)?;
    out.resize(EXOFS_PATH_MAX, 0);
    // RÈGLE 9 : copy_from_user pour pointer userspace.
    // SAFETY: out est un buffer kernel valide de longueur EXOFS_PATH_MAX.
    copy_from_user(out.as_mut_ptr(), ptr as *const u8, EXOFS_PATH_MAX)
        .map_err(|_| EFAULT)?;
    // Chercher le terminateur NUL.
    let len = out.iter().position(|&b| b == 0).unwrap_or(EXOFS_PATH_MAX);
    if len == 0 { return Err(EINVAL); }
    Ok(len)
}

/// Lit un buffer binaire depuis userspace (longueur bornée).
pub fn read_user_buf(ptr: u64, len: u64, out: &mut Vec<u8>) -> Result<(), i64> {
    let len = len as usize;
    if ptr == 0 { return Err(EFAULT); }
    if len == 0 { return Err(EINVAL); }
    if len > EXOFS_BLOB_MAX { return Err(ERANGE); }
    out.clear();
    out.try_reserve(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0);
    // RÈGLE 9 : copy_from_user.
    copy_from_user(out.as_mut_ptr(), ptr as *const u8, len)
        .map_err(|_| EFAULT)?;
    Ok(())
}

/// Écrit un buffer kernel vers userspace.
pub fn write_user_buf(dst: u64, src: &[u8]) -> Result<(), i64> {
    if dst == 0 { return Err(EFAULT); }
    copy_to_user(dst as *mut u8, src.as_ptr(), src.len())
        .map_err(|_| EFAULT)?;
    Ok(())
}
