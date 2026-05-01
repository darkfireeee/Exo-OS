//! # syscall/handlers/fs_posix.rs — Thin wrappers POSIX FS de compatibilité
//!
//! RÈGLE SYS-03 : thin wrappers uniquement.
//! Ces fonctions restent hors de la table de dispatch principale, mais elles
//! doivent déléguer au même `fs_bridge` que `table.rs` lorsqu'une opération
//! existe. Les vraies absences POSIX restent explicitement en `ENOSYS`.

use crate::syscall::fast_path::syscall_current_pid;
use crate::syscall::fs_bridge;
use crate::syscall::numbers::{EFAULT, EINVAL, ENOSYS};
use crate::syscall::validation::{read_user_path, validate_fd, validate_flags, USER_ADDR_MAX};

const AT_FDCWD: i32 = -100;
const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_REMOVEDIR: u64 = 0x200;
const OPEN_FLAGS_MASK: u64 = 0x0040_1FFF;

#[inline]
fn current_pid() -> u32 {
    syscall_current_pid()
}

#[inline]
fn supports_at_dirfd(dirfd: i32, path: &[u8]) -> bool {
    dirfd == AT_FDCWD || path.starts_with(b"/")
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler : stat / fstat / lstat / newfstatat
// ─────────────────────────────────────────────────────────────────────────────

/// `stat(path, stat_ptr)` → 0 ou errno.
///
/// Séquence : PATH_RESOLVE(path) → ObjectId → OBJECT_STAT(oid) → copy_to_user(stat).
/// LIB-05 : ObjectId n'est PAS exposé à l'appelant.
pub fn sys_stat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    fs_bridge::bridge_result(fs_bridge::fs_stat(path.as_bytes(), stat_ptr, current_pid()))
}

/// `lstat(path, stat_ptr)` — ne suit pas les liens symboliques.
pub fn sys_lstat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    fs_bridge::bridge_result(fs_bridge::fs_lstat(
        path.as_bytes(),
        stat_ptr,
        current_pid(),
    ))
}

/// `newfstatat(dirfd, path, stat_ptr, flags)`.
pub fn sys_newfstatat(
    dirfd: u64,
    path_ptr: u64,
    stat_ptr: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return EINVAL;
    }
    if !supports_at_dirfd(dirfd as i32, path.as_bytes()) {
        return EINVAL;
    }
    if flags & AT_SYMLINK_NOFOLLOW != 0 {
        fs_bridge::bridge_result(fs_bridge::fs_lstat(
            path.as_bytes(),
            stat_ptr,
            current_pid(),
        ))
    } else {
        fs_bridge::bridge_result(fs_bridge::fs_stat(path.as_bytes(), stat_ptr, current_pid()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler : mkdir / rmdir / unlink / rename
// ─────────────────────────────────────────────────────────────────────────────

/// `mkdir(path, mode)` → 0 ou errno.
pub fn sys_mkdir(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_mkdir(
        path.as_bytes(),
        mode as u32,
        current_pid(),
    ))
}

/// `rmdir(path)` → 0 ou errno.
pub fn sys_rmdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_rmdir(path.as_bytes(), current_pid()))
}

/// `unlink(path)` → 0 ou errno.
pub fn sys_unlink(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_unlink(path.as_bytes(), current_pid()))
}

/// `rename(old, new)` → 0 ou errno.
pub fn sys_rename(old_ptr: u64, new_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(old_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new = match read_user_path(new_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (old, new);
    ENOSYS
}

/// `link(old, new)` → 0 ou errno.
pub fn sys_link(old_ptr: u64, new_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(old_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new = match read_user_path(new_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (old, new);
    ENOSYS
}

/// `symlink(target, linkpath)` → 0 ou errno.
pub fn sys_symlink(target_ptr: u64, path_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let target = match read_user_path(target_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_symlink(
        target.as_bytes(),
        path.as_bytes(),
        current_pid(),
    ))
}

/// `readlink(path, buf, bufsize)` → octets écrits ou errno.
pub fn sys_readlink(
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if bufsize == 0 {
        return EINVAL;
    }
    fs_bridge::bridge_result(fs_bridge::fs_readlink(
        path.as_bytes(),
        buf_ptr,
        bufsize as usize,
        current_pid(),
    ))
}

/// `getcwd(buf, size)` → longueur du chemin ou errno.
pub fn sys_getcwd(buf_ptr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if size == 0 {
        return EINVAL;
    }
    let _ = (buf_ptr, size);
    ENOSYS
}

/// `chdir(path)` → 0 ou errno.
pub fn sys_chdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = path;
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler : getdents64 — BUG-02 FIX
// ─────────────────────────────────────────────────────────────────────────────

/// `getdents64(fd, buf, count)` → octets remplis ou errno.
///
/// **BUG-02 FIX** : Ce syscall était absent (liste '???'). Sans lui : ls/find/opendir()
/// sont impossibles. Délègue vers le même `fs_bridge` que la table principale.
pub fn sys_getdents64(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if count == 0 {
        return EINVAL;
    }
    fs_bridge::bridge_result(fs_bridge::fs_getdents64(
        fd as u32,
        buf_ptr,
        count as usize,
        current_pid(),
    ))
}

/// `openat(dirfd, path, flags, mode)`.
pub fn sys_openat(dirfd: u64, path_ptr: u64, flags: u64, mode: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let flags = match validate_flags(flags, OPEN_FLAGS_MASK) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_openat(
        dirfd as i32,
        path.as_bytes(),
        flags as u32,
        mode as u32,
        current_pid(),
    ))
}

/// `mkdirat(dirfd, path, mode)`.
pub fn sys_mkdirat(dirfd: u64, path_ptr: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if !supports_at_dirfd(dirfd as i32, path.as_bytes()) {
        return EINVAL;
    }
    fs_bridge::bridge_result(fs_bridge::fs_mkdir(
        path.as_bytes(),
        mode as u32,
        current_pid(),
    ))
}

/// `unlinkat(dirfd, path, flags)`.
pub fn sys_unlinkat(dirfd: u64, path_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if flags & !AT_REMOVEDIR != 0 {
        return EINVAL;
    }
    if !supports_at_dirfd(dirfd as i32, path.as_bytes()) {
        return EINVAL;
    }
    if flags & AT_REMOVEDIR != 0 {
        fs_bridge::bridge_result(fs_bridge::fs_rmdir(path.as_bytes(), current_pid()))
    } else {
        fs_bridge::bridge_result(fs_bridge::fs_unlink(path.as_bytes(), current_pid()))
    }
}

/// `renameat(olddirfd, old, newdirfd, new)`.
pub fn sys_renameat(od: u64, op: u64, nd: u64, np: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(op) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new = match read_user_path(np) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (od, old, nd, new);
    ENOSYS
}

/// `chmod(path, mode)`.
pub fn sys_chmod(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (path, mode);
    ENOSYS
}

/// `chown(path, uid, gid)`.
pub fn sys_chown(path_ptr: u64, uid: u64, gid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (path, uid, gid);
    ENOSYS
}

/// `access(path, mode)`.
pub fn sys_access(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (path, mode);
    ENOSYS
}

/// `truncate(path, length)`.
pub fn sys_truncate(path_ptr: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_truncate(
        path.as_bytes(),
        length,
        current_pid(),
    ))
}

/// `ftruncate(fd, length)`.
pub fn sys_ftruncate(fd: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    fs_bridge::bridge_result(fs_bridge::fs_ftruncate(fd as u32, length, current_pid()))
}

/// `fsync(fd)`.
pub fn sys_fsync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let _ = fd;
    ENOSYS
}

/// `fdatasync(fd)`.
pub fn sys_fdatasync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let _ = fd;
    ENOSYS
}
