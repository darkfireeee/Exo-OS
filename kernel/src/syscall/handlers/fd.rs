//! # syscall/handlers/fd.rs — Thin wrappers I/O (read, write, open, close, dup, fstat)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT — zéro logique métier.
//! RÈGLE SYS-01 : copy_from_user() OBLIGATOIRE pour tout pointeur Ring3.
//! RÈGLE SYS-05 : Valider longueurs AVANT copy_from_user.
//! RÈGLE SYS-07 : verify_cap() appelé dans le handler AVANT délégation.

use crate::syscall::errno::{E2BIG, EFAULT, EINVAL, ENOSYS};
use crate::syscall::validation::{
    read_user_path, validate_fd, validate_flags, UserBuf, IO_BUF_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs d'appels (instrumentation)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU64, Ordering};
static CNT_READ: AtomicU64 = AtomicU64::new(0);
static CNT_WRITE: AtomicU64 = AtomicU64::new(0);
static CNT_OPEN: AtomicU64 = AtomicU64::new(0);
static CNT_CLOSE: AtomicU64 = AtomicU64::new(0);
static CNT_DUP: AtomicU64 = AtomicU64::new(0);
static CNT_FSTAT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Flags open() autorisés (O_RDONLY/O_WRONLY/O_RDWR | O_CREAT | O_EXCL | etc.)
// ─────────────────────────────────────────────────────────────────────────────

const OPEN_FLAGS_MASK: u64 = 0x0040_1FFF;

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `read(fd, buf, count)` → octets lus ou errno.
///
/// SYS-01 : valide UserBuf avant délégation.
/// SYS-05 : rejettte count=0 (EINVAL) et count>IO_BUF_MAX (E2BIG).
pub fn sys_read(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_READ.fetch_add(1, Ordering::Relaxed);
    if count == 0 {
        return EINVAL;
    }
    if count as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let buf = match UserBuf::validate(buf_ptr, count as usize, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // Délègue → fd::io::do_read() (câblé lors de l'intégration fs/)
    let _ = (fd, buf);
    ENOSYS
}

/// `write(fd, buf, count)` → octets écrits ou errno.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_WRITE.fetch_add(1, Ordering::Relaxed);
    if count == 0 {
        return EINVAL;
    }
    if count as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let buf = match UserBuf::validate(buf_ptr, count as usize, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // Délègue → fd::io::do_write()
    let _ = (fd, buf);
    ENOSYS
}

/// `open(path, flags, mode)` → fd ou errno.
///
/// NOTE : musl-exo doit pointer __NR_open vers SYS_EXOFS_OPEN_BY_PATH (519),
/// pas ce syscall (BUG-01). Ce handler reste pour compatibilité POSIX directe.
pub fn sys_open(path_ptr: u64, flags: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_OPEN.fetch_add(1, Ordering::Relaxed);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let flags = match validate_flags(flags, OPEN_FLAGS_MASK) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    // Délègue → fs/exofs/syscall/open_by_path (combiné : PATH_RESOLVE + OBJECT_OPEN)
    let _ = (path, flags, mode);
    ENOSYS
}

/// `close(fd)` → 0 ou errno.
pub fn sys_close(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_CLOSE.fetch_add(1, Ordering::Relaxed);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    // Délègue → fd::table::close_fd()
    let _ = fd;
    ENOSYS
}

/// `dup(oldfd)` → nouveau fd ou errno.
pub fn sys_dup(oldfd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_DUP.fetch_add(1, Ordering::Relaxed);
    let fd = match validate_fd(oldfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let _ = fd;
    ENOSYS
}

/// `dup2(oldfd, newfd)` → newfd ou errno.
pub fn sys_dup2(oldfd: u64, newfd: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_DUP.fetch_add(1, Ordering::Relaxed);
    let old = match validate_fd(oldfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let new = match validate_fd(newfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let _ = (old, new);
    ENOSYS
}

/// `fstat(fd, stat_buf_ptr)` → 0 ou errno.
pub fn sys_fstat(fd: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    CNT_FSTAT.fetch_add(1, Ordering::Relaxed);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 {
        return EFAULT;
    }
    // Valider l'adresse stat_ptr (pointeur userspace — SYS-01)
    if stat_ptr >= crate::syscall::validation::USER_ADDR_MAX {
        return EFAULT;
    }
    // Délègue → fs/exofs/syscall/object_stat via fd→ObjectId
    let _ = (fd, stat_ptr);
    ENOSYS
}

/// `lseek(fd, offset, whence)` → nouvelle position ou errno.
pub fn sys_lseek(fd: u64, offset: u64, whence: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if whence > 2 {
        return EINVAL;
    }
    let _ = (fd, offset, whence);
    ENOSYS
}

/// `fcntl(fd, cmd, arg)`.
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let _ = (fd, cmd, arg);
    ENOSYS
}

/// `pread64(fd, buf, count, offset)`.
pub fn sys_pread64(fd: u64, buf_ptr: u64, count: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    if count == 0 || count as usize > IO_BUF_MAX {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let buf = match UserBuf::validate(buf_ptr, count as usize, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    let _ = (fd, buf, offset);
    ENOSYS
}

/// `pwrite64(fd, buf, count, offset)`.
pub fn sys_pwrite64(fd: u64, buf_ptr: u64, count: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    if count == 0 || count as usize > IO_BUF_MAX {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let buf = match UserBuf::validate(buf_ptr, count as usize, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    let _ = (fd, buf, offset);
    ENOSYS
}
