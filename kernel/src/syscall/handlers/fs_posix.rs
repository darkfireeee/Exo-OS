//! # syscall/handlers/fs_posix.rs — Thin wrappers POSIX FS (stat, mkdir, unlink, getdents64)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! RÈGLE SYS-07 : verify_cap() appelé AVANT délégation vers fs/exofs/.
//! RÈGLE LIB-04 : Pal::open() = 2 syscalls — PATH_RESOLVE → ObjectId → OBJECT_OPEN.
//! RÈGLE LIB-05 : INTERDIT d'exposer ObjectId dans l'API POSIX.
//! BUG-02 FIX   : SYS_EXOFS_READDIR (520) utilisé pour getdents64.


use crate::syscall::validation::{read_user_path, validate_fd, USER_ADDR_MAX};
use crate::syscall::errno::{EINVAL, EFAULT, ENOSYS};
use crate::fs::exofs::syscall;

// ─────────────────────────────────────────────────────────────────────────────
// Handler : stat / fstat / lstat / newfstatat
// ─────────────────────────────────────────────────────────────────────────────

/// `stat(path, stat_ptr)` → 0 ou errno.
///
/// Séquence : PATH_RESOLVE(path) → ObjectId → OBJECT_STAT(oid) → copy_to_user(stat).
/// LIB-05 : ObjectId n'est PAS exposé à l'appelant.
pub fn sys_stat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX { return EFAULT; }
    // SYS-07 : verify_cap() implicite dans path_resolve (vérifie READ sur le parent)
    // Délègue → fs/exofs/syscall/path_resolve puis object_stat
    let _ = (path, stat_ptr);
    ENOSYS
}

/// `lstat(path, stat_ptr)` — ne suit pas les liens symboliques.
pub fn sys_lstat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX { return EFAULT; }
    let _ = (path, stat_ptr);
    ENOSYS
}

/// `newfstatat(dirfd, path, stat_ptr, flags)`.
pub fn sys_newfstatat(
    dirfd:    u64,
    path_ptr: u64,
    stat_ptr: u64,
    flags:    u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    if stat_ptr == 0 || stat_ptr >= USER_ADDR_MAX { return EFAULT; }
    let _ = (dirfd, path, stat_ptr, flags);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler : mkdir / rmdir / unlink / rename
// ─────────────────────────────────────────────────────────────────────────────

/// `mkdir(path, mode)` → 0 ou errno.
pub fn sys_mkdir(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    // SYS-07 : verify_cap(WRITE) sur répertoire parent avant création
    let _ = (path, mode);
    ENOSYS
}

/// `rmdir(path)` → 0 ou errno.
pub fn sys_rmdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = path;
    ENOSYS
}

/// `unlink(path)` → 0 ou errno.
pub fn sys_unlink(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    // Délègue → fs/exofs/syscall/object_delete
    let _ = path;
    ENOSYS
}

/// `rename(old, new)` → 0 ou errno.
pub fn sys_rename(old_ptr: u64, new_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(old_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let new = match read_user_path(new_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (old, new);
    ENOSYS
}

/// `link(old, new)` → 0 ou errno.
pub fn sys_link(old_ptr: u64, new_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(old_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let new = match read_user_path(new_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (old, new);
    ENOSYS
}

/// `symlink(target, linkpath)` → 0 ou errno.
pub fn sys_symlink(target_ptr: u64, path_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let target = match read_user_path(target_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let path   = match read_user_path(path_ptr)   { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (target, path);
    ENOSYS
}

/// `readlink(path, buf, bufsize)` → octets écrits ou errno.
pub fn sys_readlink(path_ptr: u64, buf_ptr: u64, bufsize: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX { return EFAULT; }
    if bufsize == 0 { return EINVAL; }
    let _ = (path, buf_ptr, bufsize);
    ENOSYS
}

/// `getcwd(buf, size)` → longueur du chemin ou errno.
pub fn sys_getcwd(buf_ptr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX { return EFAULT; }
    if size == 0 { return EINVAL; }
    let _ = (buf_ptr, size);
    ENOSYS
}

/// `chdir(path)` → 0 ou errno.
pub fn sys_chdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = path;
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler : getdents64 — BUG-02 FIX
// ─────────────────────────────────────────────────────────────────────────────

/// `getdents64(fd, buf, count)` → octets remplis ou errno.
///
/// **BUG-02 FIX** : Ce syscall était absent (liste '???'). Sans lui : ls/find/opendir()
/// sont impossibles. Délègue vers SYS_EXOFS_READDIR (520).
pub fn sys_getdents64(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX { return EFAULT; }
    if count == 0 { return EINVAL; }
    // Délègue → fs/exofs/syscall/readdir (SYS_EXOFS_READDIR = 520)
    syscall::sys_exofs_readdir(fd as u64, buf_ptr, count, 0, 0, 0)
}

/// `openat(dirfd, path, flags, mode)`.
pub fn sys_openat(dirfd: u64, path_ptr: u64, flags: u64, mode: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (dirfd, path, flags, mode);
    ENOSYS
}

/// `mkdirat(dirfd, path, mode)`.
pub fn sys_mkdirat(dirfd: u64, path_ptr: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (dirfd, path, mode);
    ENOSYS
}

/// `unlinkat(dirfd, path, flags)`.
pub fn sys_unlinkat(dirfd: u64, path_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (dirfd, path, flags);
    ENOSYS
}

/// `renameat(olddirfd, old, newdirfd, new)`.
pub fn sys_renameat(od: u64, op: u64, nd: u64, np: u64, _a5: u64, _a6: u64) -> i64 {
    let old = match read_user_path(op) { Ok(p) => p, Err(e) => return e.to_errno() };
    let new = match read_user_path(np) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (od, old, nd, new);
    ENOSYS
}

/// `chmod(path, mode)`.
pub fn sys_chmod(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (path, mode);
    ENOSYS
}

/// `chown(path, uid, gid)`.
pub fn sys_chown(path_ptr: u64, uid: u64, gid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (path, uid, gid);
    ENOSYS
}

/// `access(path, mode)`.
pub fn sys_access(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (path, mode);
    ENOSYS
}

/// `truncate(path, length)`.
pub fn sys_truncate(path_ptr: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let _ = (path, length);
    ENOSYS
}

/// `ftruncate(fd, length)`.
pub fn sys_ftruncate(fd: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = (fd, length);
    ENOSYS
}

/// `fsync(fd)`.
pub fn sys_fsync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = fd;
    ENOSYS
}

/// `fdatasync(fd)`.
pub fn sys_fdatasync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = fd;
    ENOSYS
}
