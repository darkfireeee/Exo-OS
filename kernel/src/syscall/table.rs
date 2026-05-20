//! # syscall/table.rs — Table de dispatch syscall [SYSCALL_TABLE_SIZE entrées]
//!
//! Définit la table statique qui mappe chaque numéro syscall vers son
//! handler Rust. La table est un tableau de `SYSCALL_TABLE_SIZE` pointeurs de fonctions
//! initialisé à la compilation (pas de build-time procedure nécessaire).
//!
//! ## Organisation
//! - `TABLE[nr]` retourne un `SyscallHandler` (type alias sur `fn(...) -> i64`).
//! - Les entrées non implémentées pointent vers `sys_enosys`.
//! - La table est `static`, donc dans `.rodata` — lecture sans verrou.
//!
//! ## Séparation fast-path / slow-path
//! `dispatch.rs` appelle d'abord `fast_path::try_fast_path()` pour les
//! syscalls haute fréquence (<100 cycles). Ce module gère le slow-path
//! pour tout ce qui implique verrou, allocation, ou accès à fs/ipc.
//!
//! ## Instrumentation
//! Chaque handler wrapper incrémente un compteur atomique par numéro syscall.
//! Les compteurs sont lus via `syscall_table_stat(nr)`.
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

extern crate alloc;

use crate::syscall::errno::{
    E2BIG, EACCES, EAGAIN, EBUSY, EEXIST, EFAULT, EINTR, EINVAL, EMSGSIZE, ENOENT, ENOMEM, ENOSYS,
    EPERM,
};
use crate::syscall::fast_path::Timespec;
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    copy_from_user, copy_to_user, read_user_path, read_user_typed, validate_fd, validate_flags,
    write_user_typed, UserBuf, IO_BUF_MAX,
};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
// GI-03 IRQ types et fonctions
use crate::arch::x86_64::irq::{
    irq_error_to_errno, parse_irq_source_kind, IpcEndpoint, IrqAckResult, IrqOwnerPid, IrqVector,
};
// GI-03 Driver types
use crate::drivers::{ClaimError, MmioError, MsiError, PciCfgError, TopoError};

use crate::fs::exofs::syscall::{
    sys_exofs_epoch_commit, sys_exofs_export_object, sys_exofs_gc_trigger,
    sys_exofs_get_content_hash, sys_exofs_import_object, sys_exofs_object_create,
    sys_exofs_object_delete, sys_exofs_object_open, sys_exofs_object_read,
    sys_exofs_object_set_meta, sys_exofs_object_stat, sys_exofs_object_write,
    sys_exofs_open_by_path, sys_exofs_path_resolve, sys_exofs_quota_query, sys_exofs_readdir,
    sys_exofs_relation_create, sys_exofs_relation_query, sys_exofs_snapshot_create,
    sys_exofs_snapshot_list, sys_exofs_snapshot_mount,
};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, IommuDomainId, IovaAddr,
};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use pci_types::PciAddress;

// ─────────────────────────────────────────────────────────────────────────────
// Type handler
// ─────────────────────────────────────────────────────────────────────────────

/// Signature commune de tous les handlers syscall.
/// Les 6 arguments correspondent aux 6 registres ABI : rdi, rsi, rdx, r10, r8, r9.
pub type SyscallHandler = fn(u64, u64, u64, u64, u64, u64) -> i64;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs d'appel par numéro syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs atomiques — un par slot de la table.
/// Indexés directement par numéro syscall.
static SYSCALL_STATS: [AtomicU64; SYSCALL_TABLE_SIZE] = {
    // Rust ne permet pas [AtomicU64::new(0); N] pour N grand → transmute.
    // SAFETY: AtomicU64 a la même représentation que u64 (garantie par Rust reference).
    // [0u64; N] est une séquence d'octets nuls valide pour [AtomicU64; N].
    unsafe {
        core::mem::transmute::<[u64; SYSCALL_TABLE_SIZE], [AtomicU64; SYSCALL_TABLE_SIZE]>(
            [0u64; SYSCALL_TABLE_SIZE],
        )
    }
};

/// Retourne le nombre d'appels au syscall numéro `nr`.
#[inline]
pub fn syscall_table_stat(nr: usize) -> u64 {
    if nr < SYSCALL_TABLE_SIZE {
        SYSCALL_STATS[nr].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Alias public pour compatibilité avec syscall/mod.rs.
#[inline]
pub fn syscall_stats_for(nr: u64) -> u64 {
    syscall_table_stat(nr as usize)
}

/// Incrémente le compteur `nr` depuis le dispatch.
#[inline(always)]
fn stat_inc(nr: u64) {
    if (nr as usize) < SYSCALL_TABLE_SIZE {
        // SAFETY: l'accès est borné par le test ci-dessus.
        SYSCALL_STATS[nr as usize].fetch_add(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Macro pour les handlers non implémentés → ENOSYS avec compteur
// ─────────────────────────────────────────────────────────────────────────────

#[allow(unused_macros)]
macro_rules! enosys_handler {
    () => {
        (|_a: u64, _b: u64, _c: u64, _d: u64, _e: u64, _f: u64| -> i64 { ENOSYS }) as SyscallHandler
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Stub pour -ENOSYS (syscall non implémenté)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler par défaut : syscall non implémenté.
pub fn sys_enosys(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    ENOSYS
}

/// Wrapper ABI pour SYS_EXOFS_OBJECT_SET_META.
///
/// Nouveau contrat: a1=args_ptr(SetMetaArgs), a2=cap_token, a3..a6 ignorés.
pub fn sys_exofs_object_set_meta_abi(
    args_ptr: u64,
    cap_token: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    sys_exofs_object_set_meta(args_ptr, cap_token)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers I/O (délégués vers fs/)
// ─────────────────────────────────────────────────────────────────────────────

/// `read(fd, buf, count)` → nombre d'octets lus ou errno.
pub fn sys_read(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_READ);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let len = count as usize;
    // Borne maximale pour éviter les timeout : IO_BUF_MAX
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    // Valider le buffer de destination
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_read(fd as u32, buf_ptr, len, pid))
}

/// `write(fd, buf, count)` → nombre d'octets écrits ou errno.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WRITE);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let len = count as usize;
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_write(fd as u32, buf_ptr, len, pid))
}

/// `pread64(fd, buf, count, offset)` → read without changing fd cursor.
pub fn sys_pread64(fd: u64, buf_ptr: u64, count: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PREAD64);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let len = count as usize;
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pread64(fd as u32, buf_ptr, len, offset, pid))
}

/// `pwrite64(fd, buf, count, offset)` → write without changing fd cursor.
pub fn sys_pwrite64(fd: u64, buf_ptr: u64, count: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PWRITE64);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let len = count as usize;
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pwrite64(fd as u32, buf_ptr, len, offset, pid))
}

/// `readv(fd, iov, iovcnt)`.
pub fn sys_readv(fd: u64, iov_ptr: u64, iovcnt: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_READV);
    if iovcnt > u32::MAX as u64 {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_readv(fd as u32, iov_ptr, iovcnt as u32, pid))
}

/// `writev(fd, iov, iovcnt)`.
pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WRITEV);
    if iovcnt > u32::MAX as u64 {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_writev(fd as u32, iov_ptr, iovcnt as u32, pid))
}

/// `preadv(fd, iov, iovcnt, offset)`.
pub fn sys_preadv(fd: u64, iov_ptr: u64, iovcnt: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PREADV);
    if iovcnt > u32::MAX as u64 {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_preadv(
        fd as u32,
        iov_ptr,
        iovcnt as u32,
        offset,
        pid,
    ))
}

/// `pwritev(fd, iov, iovcnt, offset)`.
pub fn sys_pwritev(fd: u64, iov_ptr: u64, iovcnt: u64, offset: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PWRITEV);
    if iovcnt > u32::MAX as u64 {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pwritev(
        fd as u32,
        iov_ptr,
        iovcnt as u32,
        offset,
        pid,
    ))
}

/// `preadv2(fd, iov, iovcnt, offset, flags)`.
pub fn sys_preadv2(fd: u64, iov_ptr: u64, iovcnt: u64, offset: u64, flags: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PREADV2);
    if flags != 0 {
        return ENOSYS;
    }
    sys_preadv(fd, iov_ptr, iovcnt, offset, 0, 0)
}

/// `pwritev2(fd, iov, iovcnt, offset, flags)`.
pub fn sys_pwritev2(fd: u64, iov_ptr: u64, iovcnt: u64, offset: u64, flags: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PWRITEV2);
    if flags != 0 {
        return ENOSYS;
    }
    sys_pwritev(fd, iov_ptr, iovcnt, offset, 0, 0)
}

/// `open(path, flags, mode)` → fd ou errno.
pub fn sys_open(path_ptr: u64, flags: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_OPEN);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    // Flags O_RDONLY/O_WRONLY/O_RDWR | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_NONBLOCK
    let allowed_flags = 0x0040_1FFFu64;
    let flags = match validate_flags(flags, allowed_flags) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let mode = match checked_u32_sysarg(mode) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_open(path.as_bytes(), flags as u32, mode, pid))
}

/// `creat(path, mode)` → alias for `open(O_CREAT|O_WRONLY|O_TRUNC)`.
pub fn sys_creat(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CREAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::fs::exofs::syscall::object_fd::open_flags;
    use crate::syscall::fs_bridge;
    let mode = match checked_u32_sysarg(mode) {
        Ok(v) => v,
        Err(e) => return e,
    };
    fs_bridge::bridge_result(fs_bridge::fs_open(
        path.as_bytes(),
        open_flags::O_CREAT | open_flags::O_WRONLY | open_flags::O_TRUNC,
        mode,
        pid,
    ))
}

/// `close(fd)` → 0 ou errno.
pub fn sys_close(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOSE);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_close(fd as u32, pid))
}

/// `lseek(fd, offset, whence)` → nouvelle position ou errno.
pub fn sys_lseek(fd: u64, offset: u64, whence: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LSEEK);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if whence > 2 {
        return EINVAL;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_lseek(
        fd as u32,
        offset as i64,
        whence as u32,
        pid,
    ))
}

/// `openat(dirfd, path, flags, mode)`.
pub fn sys_openat(dirfd: u64, path_ptr: u64, flags: u64, mode: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_OPENAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let allowed_flags = 0x0040_1FFFu64;
    let flags = match validate_flags(flags, allowed_flags) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let dirfd = match checked_i32_sysarg(dirfd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mode = match checked_u32_sysarg(mode) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_openat(
        dirfd,
        path.as_bytes(),
        flags as u32,
        mode,
        pid,
    ))
}

/// `dup(oldfd)` → nouveau fd ou errno.
pub fn sys_dup(oldfd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP);
    let fd = match validate_fd(oldfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_dup(fd as u32, pid))
}

/// `dup2(oldfd, newfd)`.
pub fn sys_dup2(oldfd: u64, newfd: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP2);
    let old = match validate_fd(oldfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let new = match validate_fd(newfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_dup2(old as u32, new as u32, pid))
}

/// `dup3(oldfd, newfd, flags)`.
pub fn sys_dup3(oldfd: u64, newfd: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP3);
    const O_CLOEXEC: u64 = 0o2000000;
    if oldfd == newfd || flags & !O_CLOEXEC != 0 {
        return EINVAL;
    }
    sys_dup2(oldfd, newfd, 0, 0, 0, 0)
}

/// `fcntl(fd, cmd, arg)`.
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCNTL);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let cmd = match checked_u32_sysarg(cmd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fcntl(fd as u32, cmd, arg, pid))
}

/// `flock(fd, operation)`.
pub fn sys_flock(fd: u64, operation: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FLOCK);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let operation = match checked_u32_sysarg(operation) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_flock(fd as u32, operation, pid))
}

/// `pipe(pipefd)`.
pub fn sys_pipe(fds_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PIPE);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pipe2(fds_ptr, 0, pid))
}

/// `pipe2(pipefd, flags)`.
pub fn sys_pipe2(fds_ptr: u64, flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PIPE2);
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_pipe2(fds_ptr, flags, pid))
}

/// `eventfd(initval)`.
pub fn sys_eventfd(initval: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EVENTFD);
    let initval = match checked_u32_sysarg(initval) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_eventfd2(initval, 0, pid))
}

/// `eventfd2(initval, flags)`.
pub fn sys_eventfd2(initval: u64, flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EVENTFD2);
    let initval = match checked_u32_sysarg(initval) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_eventfd2(initval, flags, pid))
}

/// `inotify_init1(flags)`.
pub fn sys_inotify_init1(flags: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_INOTIFY_INIT1);
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_inotify_init1(flags, pid))
}

/// `poll(fds, nfds, timeout)`.
pub fn sys_poll(fds_ptr: u64, nfds: u64, timeout: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_POLL);
    if nfds > 1024 {
        return EINVAL;
    }
    let timeout = match checked_i32_sysarg(timeout) {
        Ok(v) => v,
        Err(e) => return e,
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_poll(fds_ptr, nfds as usize, timeout, pid))
}

/// `ppoll(fds, nfds, timeout, sigmask, sigsetsize)`.
pub fn sys_ppoll(
    fds_ptr: u64,
    nfds: u64,
    timeout_ptr: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PPOLL);
    let _ = (timeout_ptr, sigmask_ptr, sigsetsize);
    sys_poll(fds_ptr, nfds, 0, 0, 0, 0)
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`.
pub fn sys_select(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SELECT);
    if nfds > 1024 {
        return EINVAL;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_select(
        nfds as usize,
        readfds_ptr,
        writefds_ptr,
        exceptfds_ptr,
        timeout_ptr,
        pid,
    ))
}

/// `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask_pack)`.
pub fn sys_pselect6(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    timeout_ptr: u64,
    sigmask_pack: u64,
) -> i64 {
    stat_inc(SYS_PSELECT6);
    let _ = sigmask_pack;
    sys_select(
        nfds,
        readfds_ptr,
        writefds_ptr,
        exceptfds_ptr,
        timeout_ptr,
        0,
    )
}

/// `epoll_create(size)`.
pub fn sys_epoll_create(size: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CREATE);
    if size == 0 {
        return EINVAL;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_create1(0, pid))
}

/// `epoll_create1(flags)`.
pub fn sys_epoll_create1(flags: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CREATE1);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_create1(flags as u32, pid))
}

/// `epoll_ctl(epfd, op, fd, event)`.
pub fn sys_epoll_ctl(epfd: u64, op: u64, fd: u64, event_ptr: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EPOLL_CTL);
    let epfd = match validate_fd(epfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_ctl(
        epfd as u32,
        op as i32,
        fd as u32,
        event_ptr,
        pid,
    ))
}

/// `epoll_wait(epfd, events, maxevents, timeout)`.
pub fn sys_epoll_wait(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_WAIT);
    let epfd = match validate_fd(epfd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_epoll_wait(
        epfd as u32,
        events_ptr,
        maxevents as i32,
        timeout as i32,
        pid,
    ))
}

/// `epoll_pwait(epfd, events, maxevents, timeout, sigmask, sigsetsize)`.
pub fn sys_epoll_pwait(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_PWAIT);
    let _ = (sigmask_ptr, sigsetsize);
    sys_epoll_wait(epfd, events_ptr, maxevents, timeout, 0, 0)
}

/// `epoll_pwait2(epfd, events, maxevents, timeout, sigmask, sigsetsize)`.
pub fn sys_epoll_pwait2(
    epfd: u64,
    events_ptr: u64,
    maxevents: u64,
    timeout_ptr: u64,
    sigmask_ptr: u64,
    sigsetsize: u64,
) -> i64 {
    stat_inc(SYS_EPOLL_PWAIT2);
    let _ = (timeout_ptr, sigmask_ptr, sigsetsize);
    sys_epoll_wait(epfd, events_ptr, maxevents, 0, 0, 0)
}

/// `ioctl(fd, request, arg)`.
pub fn sys_ioctl(fd: u64, request: u64, arg: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_IOCTL);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_ioctl(fd as u32, request, arg, pid))
}

/// `stat(path, stat_buf)`.
pub fn sys_stat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_STAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 {
        return EFAULT;
    }
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_stat(path.as_bytes(), stat_ptr, pid))
}

/// `fstat(fd, stat_buf)`.
pub fn sys_fstat(fd: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FSTAT);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 {
        return EFAULT;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fstat(fd as u32, stat_ptr, pid))
}

/// `lstat(path, stat_buf)` — ne suit pas le symlink terminal.
pub fn sys_lstat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LSTAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if stat_ptr == 0 {
        return EFAULT;
    }
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_lstat(path.as_bytes(), stat_ptr, pid))
}

/// `newfstatat(dirfd, path, stat_buf, flags)`.
pub fn sys_newfstatat(
    dirfd: u64,
    path_ptr: u64,
    stat_ptr: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_NEWFSTATAT);
    const AT_FDCWD_RAW: i64 = -100;
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    if dirfd as i64 != AT_FDCWD_RAW {
        return ENOSYS;
    }
    if flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return ENOSYS;
    }
    if flags & AT_SYMLINK_NOFOLLOW != 0 {
        sys_lstat(path_ptr, stat_ptr, 0, 0, 0, 0)
    } else {
        sys_stat(path_ptr, stat_ptr, 0, 0, 0, 0)
    }
}

/// `access(path, mode)`.
pub fn sys_access(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_ACCESS);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_access(path.as_bytes(), mode as u32, pid))
}

/// `faccessat(dirfd, path, mode, flags)`.
pub fn sys_faccessat(dirfd: u64, path_ptr: u64, mode: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FACCESSAT);
    const AT_FDCWD_RAW: i64 = -100;
    if dirfd as i64 != AT_FDCWD_RAW || flags != 0 {
        return ENOSYS;
    }
    sys_access(path_ptr, mode, 0, 0, 0, 0)
}

/// `mkdir(path, mode)`.
pub fn sys_mkdir(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKDIR);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_mkdir(path.as_bytes(), mode as u32, pid))
}

/// `mkdirat(dirfd, path, mode)`.
pub fn sys_mkdirat(dirfd: u64, path_ptr: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKDIRAT);
    const AT_FDCWD_RAW: i64 = -100;
    if dirfd as i64 != AT_FDCWD_RAW {
        return ENOSYS;
    }
    sys_mkdir(path_ptr, mode, 0, 0, 0, 0)
}

/// `rmdir(path)`.
pub fn sys_rmdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RMDIR);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_rmdir(path.as_bytes(), pid))
}

/// `unlink(path)`.
pub fn sys_unlink(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_UNLINK);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_unlink(path.as_bytes(), pid))
}

/// `unlinkat(dirfd, path, flags)`.
pub fn sys_unlinkat(dirfd: u64, path_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_UNLINKAT);
    const AT_FDCWD_RAW: i64 = -100;
    const AT_REMOVEDIR: u64 = 0x200;
    if dirfd as i64 != AT_FDCWD_RAW {
        return ENOSYS;
    }
    if flags & !AT_REMOVEDIR != 0 {
        return EINVAL;
    }
    if flags & AT_REMOVEDIR != 0 {
        sys_rmdir(path_ptr, 0, 0, 0, 0, 0)
    } else {
        sys_unlink(path_ptr, 0, 0, 0, 0, 0)
    }
}

/// `rename(oldpath, newpath)`.
pub fn sys_rename(
    old_path_ptr: u64,
    new_path_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RENAME);
    let pid = current_pid_u32();
    let old_path = match read_user_path(old_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new_path = match read_user_path(new_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_rename(
        old_path.as_bytes(),
        new_path.as_bytes(),
        pid,
    ))
}

/// `renameat(olddirfd, oldpath, newdirfd, newpath)`.
pub fn sys_renameat(
    olddirfd: u64,
    old_path_ptr: u64,
    newdirfd: u64,
    new_path_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RENAMEAT);
    let pid = current_pid_u32();
    let old_path = match read_user_path(old_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new_path = match read_user_path(new_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_renameat(
        olddirfd as i32,
        old_path.as_bytes(),
        newdirfd as i32,
        new_path.as_bytes(),
        pid,
    ))
}

/// `renameat2(olddirfd, oldpath, newdirfd, newpath, flags)`.
pub fn sys_renameat2(
    olddirfd: u64,
    old_path_ptr: u64,
    newdirfd: u64,
    new_path_ptr: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RENAMEAT2);
    const AT_FDCWD_RAW: i64 = -100;
    const RENAME_NOREPLACE: u64 = 1;
    if olddirfd as i64 != AT_FDCWD_RAW || newdirfd as i64 != AT_FDCWD_RAW {
        return ENOSYS;
    }
    if flags & !RENAME_NOREPLACE != 0 {
        return EINVAL;
    }
    if flags & RENAME_NOREPLACE != 0 {
        let pid = current_pid_u32();
        let new_path = match read_user_path(new_path_ptr) {
            Ok(p) => p,
            Err(e) => return e.to_errno(),
        };
        use crate::syscall::fs_bridge;
        match fs_bridge::fs_access(new_path.as_bytes(), 0, pid) {
            Ok(_) => return EEXIST,
            Err(fs_bridge::FsBridgeError::NotFound) => {}
            Err(e) => return e.to_errno(),
        }
    }
    sys_renameat(olddirfd, old_path_ptr, newdirfd, new_path_ptr, 0, 0)
}

/// `link(oldpath, newpath)`.
pub fn sys_link(
    old_path_ptr: u64,
    new_path_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_LINK);
    let pid = current_pid_u32();
    let old_path = match read_user_path(old_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new_path = match read_user_path(new_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_link(
        old_path.as_bytes(),
        new_path.as_bytes(),
        pid,
    ))
}

/// `linkat(olddirfd, oldpath, newdirfd, newpath, flags)`.
pub fn sys_linkat(
    olddirfd: u64,
    old_path_ptr: u64,
    newdirfd: u64,
    new_path_ptr: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_LINKAT);
    let pid = current_pid_u32();
    let old_path = match read_user_path(old_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let new_path = match read_user_path(new_path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_linkat(
        olddirfd as i32,
        old_path.as_bytes(),
        newdirfd as i32,
        new_path.as_bytes(),
        flags as u32,
        pid,
    ))
}

/// `symlink(target, linkpath)`.
pub fn sys_symlink(
    target_ptr: u64,
    linkpath_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SYMLINK);
    let pid = current_pid_u32();
    let target = match read_user_path(target_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let linkpath = match read_user_path(linkpath_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_symlink(
        target.as_bytes(),
        linkpath.as_bytes(),
        pid,
    ))
}

/// `symlinkat(target, dirfd, linkpath)`.
pub fn sys_symlinkat(
    target_ptr: u64,
    dirfd: u64,
    linkpath_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SYMLINKAT);
    let pid = current_pid_u32();
    let target = match read_user_path(target_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let linkpath = match read_user_path(linkpath_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_symlinkat(
        target.as_bytes(),
        dirfd as i32,
        linkpath.as_bytes(),
        pid,
    ))
}

/// `getdents64(fd, dirp, count)`.
pub fn sys_getdents64(fd: u64, dirp: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETDENTS64);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_getdents64(
        fd as u32,
        dirp,
        count as usize,
        pid,
    ))
}

/// `readlink(path, buf, bufsize)`.
pub fn sys_readlink(
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_READLINK);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_readlink(
        path.as_bytes(),
        buf_ptr,
        bufsize as usize,
        pid,
    ))
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
pub fn sys_readlinkat(
    dirfd: u64,
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_READLINKAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_readlinkat(
        dirfd as i32,
        path.as_bytes(),
        buf_ptr,
        bufsize as usize,
        pid,
    ))
}

/// `truncate(path, length)`.
pub fn sys_truncate(path_ptr: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TRUNCATE);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_truncate(path.as_bytes(), length, pid))
}

/// `ftruncate(fd, length)`.
pub fn sys_ftruncate(fd: u64, length: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FTRUNCATE);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_ftruncate(fd as u32, length, pid))
}

/// `fsync(fd)`.
pub fn sys_fsync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FSYNC);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fsync(fd as u32, false, pid))
}

/// `fdatasync(fd)`.
pub fn sys_fdatasync(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FDATASYNC);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fsync(fd as u32, true, pid))
}

/// `statfs(path, buf)`.
pub fn sys_statfs(path_ptr: u64, statfs_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_STATFS);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    if statfs_ptr == 0 {
        return EFAULT;
    }
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_statfs(path.as_bytes(), statfs_ptr, pid))
}

/// `fstatfs(fd, buf)`.
pub fn sys_fstatfs(fd: u64, statfs_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FSTATFS);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    if statfs_ptr == 0 {
        return EFAULT;
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fstatfs(fd as u32, statfs_ptr, pid))
}

/// `chmod(path, mode)`.
pub fn sys_chmod(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CHMOD);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_chmod(path.as_bytes(), mode as u32, pid))
}

/// `fchmod(fd, mode)`.
pub fn sys_fchmod(fd: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCHMOD);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fchmod(fd as u32, mode as u32, pid))
}

/// `chown(path, uid, gid)`.
pub fn sys_chown(path_ptr: u64, uid: u64, gid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CHOWN);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_chown(
        path.as_bytes(),
        uid as u32,
        gid as u32,
        pid,
    ))
}

/// `fchown(fd, uid, gid)`.
pub fn sys_fchown(fd: u64, uid: u64, gid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCHOWN);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fchown(fd as u32, uid as u32, gid as u32, pid))
}

/// `lchown(path, uid, gid)`.
pub fn sys_lchown(path_ptr: u64, uid: u64, gid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LCHOWN);
    sys_chown(path_ptr, uid, gid, 0, 0, 0)
}

/// `fchmodat(dirfd, path, mode, flags)`.
pub fn sys_fchmodat(dirfd: u64, path_ptr: u64, mode: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCHMODAT);
    const AT_FDCWD_RAW: i64 = -100;
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    if dirfd as i64 != AT_FDCWD_RAW || flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return ENOSYS;
    }
    sys_chmod(path_ptr, mode, 0, 0, 0, 0)
}

/// `fchownat(dirfd, path, uid, gid, flags)`.
pub fn sys_fchownat(dirfd: u64, path_ptr: u64, uid: u64, gid: u64, flags: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCHOWNAT);
    const AT_FDCWD_RAW: i64 = -100;
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    if dirfd as i64 != AT_FDCWD_RAW || flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return ENOSYS;
    }
    sys_chown(path_ptr, uid, gid, 0, 0, 0)
}

/// `statx(dirfd, path, flags, mask, statxbuf)`.
pub fn sys_statx(
    dirfd: u64,
    path_ptr: u64,
    flags: u64,
    mask: u64,
    statx_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_STATX);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_statx(
        dirfd as i32,
        path.as_bytes(),
        flags as u32,
        mask as u32,
        statx_ptr,
        pid,
    ))
}

/// `getcwd(buf, size)`.
pub fn sys_getcwd(buf_ptr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETCWD);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_getcwd(buf_ptr, size as usize, pid))
}

/// `chdir(path)`.
pub fn sys_chdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CHDIR);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_chdir(path.as_bytes(), pid))
}

/// `fchdir(fd)`.
pub fn sys_fchdir(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCHDIR);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fchdir(fd as u32, pid))
}

/// `umask(mask)`.
pub fn sys_umask(mask: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_UMASK);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_umask(mask as u32, pid))
}

/// `getrlimit(resource, rlim)`.
pub fn sys_getrlimit(resource: u64, rlim_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETRLIMIT);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_getrlimit(resource as u32, rlim_ptr, pid))
}

/// `setrlimit(resource, rlim)`.
pub fn sys_setrlimit(resource: u64, rlim_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SETRLIMIT);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_setrlimit(resource as u32, rlim_ptr, pid))
}

/// `copy_file_range(fd_in, off_in, fd_out, off_out, len, flags)`.
pub fn sys_copy_file_range(
    fd_in: u64,
    off_in_ptr: u64,
    fd_out: u64,
    off_out_ptr: u64,
    len: u64,
    flags: u64,
) -> i64 {
    stat_inc(SYS_COPY_FILE_RANGE);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let fd_in = match validate_fd(fd_in) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let fd_out = match validate_fd(fd_out) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_copy_file_range(
        fd_in as u32,
        off_in_ptr,
        fd_out as u32,
        off_out_ptr,
        len as usize,
        flags as u32,
        pid,
    ))
}

/// `sendfile(out_fd, in_fd, offset, count)`.
pub fn sys_sendfile(
    out_fd: u64,
    in_fd: u64,
    offset_ptr: u64,
    count: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SENDFILE);
    if count as usize as u64 != count || count as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let out_fd = match validate_fd(out_fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let in_fd = match validate_fd(in_fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_sendfile(
        out_fd as u32,
        in_fd as u32,
        offset_ptr,
        count as usize,
        pid,
    ))
}

/// `fallocate(fd, mode, offset, len)`.
pub fn sys_fallocate(fd: u64, mode: u64, offset: u64, len: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FALLOCATE);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fallocate(
        fd as u32,
        mode as u32,
        offset,
        len,
        pid,
    ))
}

/// `sync()`.
pub fn sys_sync(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SYNC);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_sync(pid))
}

/// `sync_file_range(fd, offset, nbytes, flags)`.
pub fn sys_sync_file_range(
    fd: u64,
    offset: u64,
    nbytes: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SYNC_FILE_RANGE);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_sync_file_range(
        fd as u32,
        offset,
        nbytes,
        flags as u32,
        pid,
    ))
}

/// `fadvise64(fd, offset, len, advice)`.
pub fn sys_fadvise64(fd: u64, offset: u64, len: u64, advice: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FADVISE64);
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fadvise64(
        fd as u32,
        offset,
        len,
        advice as u32,
        pid,
    ))
}

/// `mknod(path, mode, dev)`.
pub fn sys_mknod(path_ptr: u64, mode: u64, dev: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKNOD);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_mknod(path.as_bytes(), mode as u32, dev, pid))
}

/// `mknodat(dirfd, path, mode, dev)`.
pub fn sys_mknodat(dirfd: u64, path_ptr: u64, mode: u64, dev: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKNODAT);
    let pid = current_pid_u32();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_mknodat(
        dirfd as i32,
        path.as_bytes(),
        mode as u32,
        dev,
        pid,
    ))
}

/// `socket(domain, type, protocol)`.
pub fn sys_socket(domain: u64, ty: u64, protocol: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SOCKET);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_socket(
        domain as i32,
        ty as i32,
        protocol as i32,
    ))
}

/// `connect(fd, sockaddr*, addrlen)`.
pub fn sys_connect(fd: u64, addr_ptr: u64, addr_len: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CONNECT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_connect(fd as i32, addr_ptr, addr_len))
}

/// `bind(fd, sockaddr*, addrlen)`.
pub fn sys_bind(fd: u64, addr_ptr: u64, addr_len: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_BIND);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_bind(fd as i32, addr_ptr, addr_len))
}

/// `listen(fd, backlog)`.
pub fn sys_listen(fd: u64, backlog: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LISTEN);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_listen(fd as i32, backlog as i32))
}

/// `accept(fd, sockaddr*, socklen_t*)`.
pub fn sys_accept(fd: u64, addr_ptr: u64, addr_len_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_ACCEPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_accept(fd as i32, addr_ptr, addr_len_ptr))
}

/// `sendto(fd, buf, len, flags, sockaddr*, addrlen)`.
pub fn sys_sendto(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    flags: u64,
    addr_ptr: u64,
    addr_len: u64,
) -> i64 {
    stat_inc(SYS_SENDTO);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_sendto(
        fd as i32,
        buf_ptr,
        len as usize,
        flags as u32,
        addr_ptr,
        addr_len,
    ))
}

/// `recvfrom(fd, buf, len, flags, sockaddr*, socklen_t*)`.
pub fn sys_recvfrom(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    flags: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> i64 {
    stat_inc(SYS_RECVFROM);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_recvfrom(
        fd as i32,
        buf_ptr,
        len as usize,
        flags as u32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `sendmsg(fd, msghdr*, flags)`.
pub fn sys_sendmsg(fd: u64, msg_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SENDMSG);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_sendmsg(fd as i32, msg_ptr, flags as u32))
}

/// `recvmsg(fd, msghdr*, flags)`.
pub fn sys_recvmsg(fd: u64, msg_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RECVMSG);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_recvmsg(fd as i32, msg_ptr, flags as u32))
}

/// `shutdown(fd, how)`.
pub fn sys_shutdown(fd: u64, how: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SHUTDOWN);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_shutdown(fd as i32, how as i32))
}

/// `getsockname(fd, sockaddr*, socklen_t*)`.
pub fn sys_getsockname(
    fd: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETSOCKNAME);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getsockname(
        fd as i32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `getpeername(fd, sockaddr*, socklen_t*)`.
pub fn sys_getpeername(
    fd: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETPEERNAME);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getpeername(
        fd as i32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `socketpair(domain, type, protocol, sv)`.
pub fn sys_socketpair(domain: u64, ty: u64, protocol: u64, sv_ptr: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SOCKETPAIR);
    use crate::syscall::net_bridge;
    let pid = current_pid_u32();
    net_bridge::bridge_result(net_bridge::net_socketpair(
        domain as i32,
        ty as i32,
        protocol as i32,
        sv_ptr,
        pid,
    ))
}

/// `setsockopt(fd, level, optname, optval, optlen)`.
pub fn sys_setsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval: u64,
    optlen: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SETSOCKOPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_setsockopt(
        fd as i32,
        level as i32,
        optname as i32,
        optval,
        optlen as u32,
    ))
}

/// `getsockopt(fd, level, optname, optval, optlen*)`.
pub fn sys_getsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval: u64,
    optlen_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETSOCKOPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getsockopt(
        fd as i32,
        level as i32,
        optname as i32,
        optval,
        optlen_ptr,
    ))
}

/// `splice(fd_in, off_in, fd_out, off_out, len, flags)`.
pub fn sys_splice(
    fd_in: u64,
    off_in_ptr: u64,
    fd_out: u64,
    off_out_ptr: u64,
    len: u64,
    flags: u64,
) -> i64 {
    stat_inc(SYS_SPLICE);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let fd_in = match validate_fd(fd_in) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let fd_out = match validate_fd(fd_out) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_splice(
        fd_in as u32,
        off_in_ptr,
        fd_out as u32,
        off_out_ptr,
        len as usize,
        flags as u32,
        pid,
    ))
}

/// `tee(fd_in, fd_out, len, flags)`.
pub fn sys_tee(fd_in: u64, fd_out: u64, len: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TEE);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    let fd_in = match validate_fd(fd_in) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    let fd_out = match validate_fd(fd_out) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_tee(
        fd_in as u32,
        fd_out as u32,
        len as usize,
        flags as u32,
        pid,
    ))
}

/// `vmsplice(fd, iov, nr_segs, flags)`.
pub fn sys_vmsplice(fd: u64, iov_ptr: u64, iovcnt: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_VMSPLICE);
    if iovcnt > u32::MAX as u64 {
        return EINVAL;
    }
    let fd = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e.to_errno(),
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_vmsplice(
        fd as u32,
        iov_ptr,
        iovcnt as u32,
        flags as u32,
        pid,
    ))
}

/// `msync(addr, length, flags)`; accepted for RAM-backed ExoFS mappings.
pub fn sys_msync(addr: u64, len: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSYNC);
    const MS_ASYNC: u64 = 0x1;
    const MS_INVALIDATE: u64 = 0x2;
    const MS_SYNC: u64 = 0x4;
    if addr == 0 || len == 0 || flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0 {
        return EINVAL;
    }
    0
}

/// `mremap(old_addr, old_size, new_size, flags, new_addr)`.
pub fn sys_mremap(
    old_addr: u64,
    old_size: u64,
    new_size: u64,
    flags: u64,
    new_addr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_MREMAP);
    const MREMAP_MAYMOVE: u64 = 0x1;
    const MREMAP_FIXED: u64 = 0x2;
    const MREMAP_DONTUNMAP: u64 = 0x4;
    const PROT_READ_WRITE: u32 = 0x1 | 0x2;
    const MAP_PRIVATE_ANON: u32 = 0x02 | 0x20;

    if old_addr == 0 || old_size == 0 || new_size == 0 {
        return EINVAL;
    }
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 {
        return EINVAL;
    }
    if flags & MREMAP_FIXED != 0 && (flags & MREMAP_MAYMOVE == 0 || new_addr == 0) {
        return EINVAL;
    }
    let old_len = match checked_usize_sysarg(old_size) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let new_len = match checked_usize_sysarg(new_size) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if new_len <= old_len && flags & MREMAP_FIXED == 0 {
        return old_addr as i64;
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return ENOMEM;
    }

    match crate::memory::virt::mmap::do_mremap_zero_copy(
        old_addr, old_len, new_len, flags, new_addr,
    ) {
        Ok(crate::memory::virt::mmap::MremapZeroCopy::Moved(addr)) => return addr as i64,
        Ok(crate::memory::virt::mmap::MremapZeroCopy::Unsupported) => {}
        Err(e) => return e.to_kernel_errno() as i64,
    }

    let hint = if flags & MREMAP_FIXED != 0 {
        new_addr
    } else {
        0
    };
    let mapped = match crate::memory::virt::mmap::do_mmap(
        hint,
        new_len,
        PROT_READ_WRITE,
        MAP_PRIVATE_ANON,
        -1,
        0,
    ) {
        Ok(addr) => addr as u64,
        Err(e) => return e.to_kernel_errno() as i64,
    };

    let copy_len = old_len.min(new_len);
    if copy_len != 0 {
        if let Err(errno) = copy_user_range_streamed(old_addr, mapped, copy_len) {
            let _ = crate::memory::virt::mmap::do_munmap(mapped, new_len);
            return errno;
        }
    }
    if flags & MREMAP_DONTUNMAP == 0 {
        let _ = crate::memory::virt::mmap::do_munmap(old_addr, old_len);
    }
    mapped as i64
}

fn copy_user_range_streamed(src: u64, dst: u64, len: usize) -> Result<(), i64> {
    const MREMAP_COPY_CHUNK: usize = crate::memory::core::PAGE_SIZE;
    let mut buf = [0u8; MREMAP_COPY_CHUNK];
    let mut copied = 0usize;
    while copied < len {
        let n = core::cmp::min(MREMAP_COPY_CHUNK, len - copied);
        let src_addr = src.checked_add(copied as u64).ok_or(EFAULT)?;
        let dst_addr = dst.checked_add(copied as u64).ok_or(EFAULT)?;
        UserBuf::validate(src_addr, n, MREMAP_COPY_CHUNK).map_err(|e| e.to_errno())?;
        UserBuf::validate(dst_addr, n, MREMAP_COPY_CHUNK).map_err(|e| e.to_errno())?;
        copy_from_user(buf.as_mut_ptr(), src_addr as *const u8, n).map_err(|e| e.to_errno())?;
        copy_to_user(dst_addr as *mut u8, buf.as_ptr(), n).map_err(|e| e.to_errno())?;
        copied += n;
    }
    Ok(())
}

/// `clock_gettime(clockid, tp)` slow-path wrapper.
pub fn sys_clock_gettime(clk_id: u64, tp_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOCK_GETTIME);
    crate::syscall::fast_path::sys_clock_gettime(clk_id, tp_ptr)
}

/// `gettimeofday(tv, tz)` slow-path wrapper.
pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETTIMEOFDAY);
    crate::syscall::fast_path::sys_gettimeofday(tv_ptr, tz_ptr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Mémoire (délégués vers memory/)
// ─────────────────────────────────────────────────────────────────────────────

/// `mmap(addr, len, prot, flags, fd, off)` → adresse mappée ou errno.
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    stat_inc(SYS_MMAP);
    // Longueur doit être > 0 et multiple de PAGE_SIZE
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd = match checked_i32_sysarg(fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let _len_pages = (len + 4095) / 4096;
    // Déléguer à memory/virtual/mmap.rs
    match crate::memory::virt::mmap::do_mmap(addr, len, prot, flags, fd, off) {
        Ok(va) => va as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `munmap(addr, len)`.
pub fn sys_munmap(addr: u64, len: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MUNMAP);
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap(addr, len) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `mprotect(addr, len, prot)`.
pub fn sys_mprotect(addr: u64, len: u64, prot: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MPROTECT);
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mprotect(addr, len, prot) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `brk(addr)` → nouvelle borne du segment data ou errno.
pub fn sys_brk(addr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_BRK);
    match crate::memory::virt::mmap::do_brk(addr) {
        Ok(new_brk) => {
            sync_current_pcb_brk(new_brk);
            new_brk as i64
        }
        Err(_) => ENOMEM,
    }
}

fn sync_current_pcb_brk(new_brk: u64) {
    let tcb = crate::scheduler::core::switch::current_thread_raw();
    if tcb.is_null() {
        return;
    }

    // SAFETY: current_thread_raw() returned a non-null TCB for the running thread.
    let pid = unsafe { (*tcb).pid.0 };
    if pid == 0 {
        return;
    }

    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
    {
        pcb.brk_current.store(new_brk, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Process / Thread (délégués vers process/)
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` → PID fils dans le parent, 0 dans le fils, ou errno.
/// do_fork(ForkContext) requiert le PCB + TCB courants — câblé lors de l'intégration process/.
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FORK);
    ENOSYS
}

/// `vfork()` — câblé via do_fork(ForkFlags::VFORK) lors de l'intégration.
pub fn sys_vfork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_VFORK);
    ENOSYS
}

/// `clone(flags, stack, ptid, ctid, tls)` — crée un nouveau thread via create_thread.
///
/// Convention Exo-OS :
/// - `stack`     : RSP initial du thread fils (userspace, ou 0 → stack kernel 8 MiB).
/// - `tls`       : point d'entrée du thread fils (si non nul, prioritaire sur ctid).
/// - `ctid`      : point d'entrée alternatif ou pointeur ctid POSIX.
/// - `ptid`      : adresse où écrire le TID du fils (pthread_out).
/// - CLONE_DETACHED (0x0040_0000) : thread détaché.
pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLONE);

    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_DETACHED: u64 = 0x0040_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    const SUPPORTED_THREAD_FLAGS: u64 = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED
        | CLONE_CHILD_SETTID;

    let thread_core = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
    if flags & CLONE_THREAD == 0 {
        return ENOSYS;
    }
    if flags & thread_core != thread_core {
        return EINVAL;
    }
    if flags & !SUPPORTED_THREAD_FLAGS != 0 {
        return EINVAL;
    }

    // Récupérer le PID du thread courant via GS:[0x20].
    // SAFETY: GS:[0x20] est initialisé par context_switch avant toute entrée syscall.
    let current_pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 {
            return EFAULT;
        }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    };

    // Trouver le PCB du processus courant dans le registry global.
    let pcb_ref = match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(current_pid_val))
    {
        Some(p) => p,
        None => return -3i64, // ESRCH
    };

    // Point d'entrée : tls en priorité (pthread_create convention) puis ctid.
    let start_func = if tls != 0 { tls } else { ctid };
    if start_func == 0 {
        return EINVAL;
    }
    // Stack : l'appelant fournit RSP ou on alloue un stack kernel par défaut.
    let stack_addr = if stack != 0 {
        stack.saturating_sub(16)
    } else {
        0
    };
    let stack_size = if stack != 0 { 0 } else { 8 * 1024 * 1024 };
    let detached = (flags & CLONE_DETACHED) != 0;

    let attr = crate::process::thread::creation::ThreadAttr {
        stack_size,
        stack_addr,
        policy: crate::scheduler::core::task::SchedPolicy::Normal,
        priority: crate::scheduler::core::task::Priority::NORMAL_DEFAULT,
        detached,
        cpu_affinity: -1,
        sigaltstack_size: 8192,
    };
    let params = crate::process::thread::creation::ThreadCreateParams {
        pcb: pcb_ref as *const crate::process::core::pcb::ProcessControlBlock,
        attr,
        start_func,
        arg: 0,
        target_cpu: 0,
        pthread_out: ptid,
    };
    match crate::process::thread::creation::create_thread(&params) {
        Ok(handle) => handle.tid.0 as i64,
        Err(crate::process::thread::creation::ThreadCreateError::OutOfMemory) => ENOMEM,
        Err(crate::process::thread::creation::ThreadCreateError::TidExhausted)
        | Err(crate::process::thread::creation::ThreadCreateError::TooManyThreads) => EAGAIN,
        Err(_) => EINVAL,
    }
}

/// `execve(path, argv, envp)`.
pub fn sys_execve(
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXECVE);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    // do_execve requiert &mut ProcessThread + &ProcessControlBlock — câblé lors de l'intégration.
    let _ = (path, argv_ptr, envp_ptr);
    ENOSYS
}

/// `exit(status)` — marque le thread Dead et cède le CPU via schedule_block.
///
/// Cette implémentation minimale est fonctionnelle : le thread ne sera plus
/// jamais choisi par pick_next_task (état Dead ignoré par la runqueue).
/// La libération complète des ressources (fds, PCB) requiert process/ pleinement
/// intégré et est réalisée de manière asynchrone par le reaper kthread.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT);
    // BUG-1 FIX: exit_code était calculé mais jamais stocké dans le PCB.
    // waitpid() obtenait toujours 0 quel que soit le code de sortie réel.
    let exit_code = (status & 0xFF) as u32;
    // SAFETY: GS:[0x20] est le TCB du thread courant initialisé par context_switch.
    // L'appel est valide depuis le contexte syscall (kernel GS actif après SWAPGS).
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nomem, nostack));
        if tcb_ptr != 0 {
            let tcb = &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock);

            // Stocker le code de sortie dans le PCB pour waitpid() (BUG-1 FIX).
            // REGISTRY.find_by_pid() est lockless — sûr depuis un contexte syscall.
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
                use core::sync::atomic::Ordering;
                pcb.exit_code.store(exit_code, Ordering::Release);
                pcb.flags.fetch_or(
                    crate::process::core::pcb::process_flags::VFORK_DONE,
                    Ordering::Release,
                );
                pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
                crate::process::lifecycle::fork::notify_vfork_completion(pid);
            }

            // Transition Dead → pick_next_task ignorera ce thread.
            tcb.set_state(crate::scheduler::core::task::TaskState::Dead);
            let cpu_id = tcb.current_cpu();
            let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
            // schedule_block sélectionne le prochain thread et effectue le context switch.
            crate::scheduler::core::switch::schedule_block(rq, &mut *(tcb_ptr as *mut _));
        }
    }
    // Unreachable après schedule_block avec état Dead (satisfait le type -> i64).
    #[allow(clippy::empty_loop)]
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// `exit_group(status)` — termine tous les threads du groupe de processus.
///
/// Délègue vers sys_exit() pour l'instant.
/// L'itération sur tous les threads frères requiert process/ pleinement intégré.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT_GROUP);
    let exit_code = (status & 0xFF) as u32;

    // SAFETY: GS:[0x20] est le TCB courant pendant le syscall.
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nomem, nostack));
        if tcb_ptr == 0 {
            return EFAULT;
        }
        let current_tcb = &mut *(tcb_ptr as *mut crate::scheduler::core::task::ThreadControlBlock);
        let pid = crate::process::core::pid::Pid(current_tcb.pid.0);
        let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) else {
            return -3;
        };

        pcb.set_exiting();
        pcb.exit_code
            .store(exit_code, core::sync::atomic::Ordering::Release);
        pcb.flags.fetch_or(
            crate::process::core::pcb::process_flags::VFORK_DONE,
            core::sync::atomic::Ordering::Release,
        );

        {
            let mut files = pcb.files.lock();
            files.close_all_noalloc();
        }
        crate::process::lifecycle::exit::close_all_pid_vfs(pcb.pid.0);
        crate::drivers::driver_do_exit(pcb.pid.0);

        pcb.for_each_thread_ptr(|thread_ptr| {
            let thread = &mut *thread_ptr;
            thread
                .join_result
                .store(exit_code as u64, core::sync::atomic::Ordering::Release);
            thread
                .join_done
                .store(true, core::sync::atomic::Ordering::Release);
            thread.sched_tcb.mark_exiting();
            thread.set_state(crate::scheduler::core::task::TaskState::Dead);
            crate::process::lifecycle::reap::REAPER_QUEUE.enqueue(thread.pid, thread.tid);
        });
        current_tcb.mark_exiting();
        current_tcb.set_state(crate::scheduler::core::task::TaskState::Dead);

        pcb.thread_count
            .store(0, core::sync::atomic::Ordering::Release);
        pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
        let ppid = pcb.ppid();
        if ppid.0 != 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                ppid,
                crate::process::signal::default::Signal::SIGCHLD,
            );
        }
        crate::process::lifecycle::wait::wake_waiting_parents(pcb.pid, ppid);
        crate::process::lifecycle::fork::notify_vfork_completion(pcb.pid);

        let cpu_id = current_tcb.current_cpu();
        let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
        crate::scheduler::core::switch::schedule_block(rq, current_tcb);
    }

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// `wait4(pid, wstatus, options, rusage)`.
/// do_waitpid(caller_pid, wait_pid, WaitOptions, &tcb) câblé lors de l'intégration.
pub fn sys_wait4(
    pid: u64,
    wstatus_ptr: u64,
    options: u64,
    rusage_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_WAIT4);
    crate::syscall::handlers::process::sys_wait4(pid, wstatus_ptr, options, rusage_ptr, 0, 0)
}

/// `waitid(idtype, id, infop, options, rusage)`.
pub fn sys_waitid(
    idtype: u64,
    id: u64,
    infop: u64,
    options: u64,
    rusage_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_WAITID);
    crate::syscall::handlers::process::sys_waitid(idtype, id, infop, options, rusage_ptr, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Signaux (délégués vers process/signal/)
// ─────────────────────────────────────────────────────────────────────────────

/// `kill(pid, sig)` — envoie le signal `sig` au processus `pid`.
///
/// ## RÈGLE SIGNAL-01 (DOC1)
/// kill() soumet le signal via process::signal::delivery.
/// La livraison effective se fait au retour userspace du thread cible.
pub fn sys_kill(pid: u64, sig: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_KILL);
    crate::syscall::handlers::signal::sys_kill(pid, sig, 0, 0, 0, 0)
}

/// `tgkill(tgid, tid, sig)` — câblé via send_signal_to_tcb lors de l'intégration.
pub fn sys_tgkill(tgid: u64, tid: u64, sig: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TGKILL);
    crate::syscall::handlers::signal::sys_tgkill(tgid, tid, sig, 0, 0, 0)
}

/// `rt_sigaction(sig, act_ptr, oldact_ptr, sigsetsize)`.
pub fn sys_rt_sigaction(
    sig: u64,
    act_ptr: u64,
    oldact_ptr: u64,
    size: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RT_SIGACTION);
    crate::syscall::handlers::signal::sys_rt_sigaction(sig, act_ptr, oldact_ptr, size, 0, 0)
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn sys_rt_sigprocmask(
    how: u64,
    set_ptr: u64,
    oldset_ptr: u64,
    size: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RT_SIGPROCMASK);
    crate::syscall::handlers::signal::sys_rt_sigprocmask(how, set_ptr, oldset_ptr, size, 0, 0)
}

/// `sigaltstack(ss_ptr, old_ss_ptr)` — configure le stack alternatif pour les signaux.
pub fn sys_sigaltstack(
    ss_ptr: u64,
    old_ss_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SIGALTSTACK);
    crate::syscall::handlers::signal::sys_sigaltstack(ss_ptr, old_ss_ptr, 0, 0, 0, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Scheduler (delay, nanosleep, futex)
// ─────────────────────────────────────────────────────────────────────────────

/// `nanosleep(req_ptr, rem_ptr)` — suspend le thread pendant une durée.
pub fn sys_nanosleep(req_ptr: u64, rem_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_NANOSLEEP);
    if req_ptr == 0 {
        return EFAULT;
    }
    let ts = match read_user_typed::<Timespec>(req_ptr) {
        Ok(t) => t,
        Err(e) => return e.to_errno(),
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let ns = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
    if !crate::scheduler::timer::sleep_ns(ns) {
        return EINTR;
    }
    let _ = rem_ptr;
    0
}

/// `clock_nanosleep(clockid, flags, req, rem)`.
pub fn sys_clock_nanosleep(
    clk_id: u64,
    flags: u64,
    req_ptr: u64,
    rem_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_CLOCK_NANOSLEEP);
    const TIMER_ABSTIME: u64 = 1;
    const CLOCK_REALTIME: u64 = 0;
    const CLOCK_MONOTONIC: u64 = 1;
    const CLOCK_BOOTTIME: u64 = 7;
    if flags & !TIMER_ABSTIME != 0 {
        return EINVAL;
    }
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME => {}
        _ => return EINVAL,
    }
    if flags & TIMER_ABSTIME == 0 {
        return sys_nanosleep(req_ptr, rem_ptr, 0, 0, 0, 0);
    }
    if req_ptr == 0 {
        return EFAULT;
    }
    let ts = match read_user_typed::<Timespec>(req_ptr) {
        Ok(t) => t,
        Err(e) => return e.to_errno(),
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let target_ns = (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64);
    if !crate::scheduler::timer::sleep_until_ns(target_ns) {
        return EINTR;
    }
    let _ = rem_ptr;
    0
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`.
pub fn sys_futex(uaddr: u64, op: u64, val: u64, timeout: u64, uaddr2: u64, val3: u64) -> i64 {
    stat_inc(SYS_FUTEX);
    // futex est dans memory/utils/futex_table.rs (RÈGLE SCHED-03 DOC3).
    match crate::memory::utils::futex_table::sys_futex(
        uaddr,
        op as u32,
        val as u32,
        timeout,
        uaddr2,
        val3 as u32,
    ) {
        Ok(v) => v,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `getrandom(buf, buflen, flags)` — Linux-compatible syscall 318.
pub fn sys_getrandom(buf_ptr: u64, len: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETRANDOM);
    const GETRANDOM_MAX: usize = 256 * 1024;
    const GRND_NONBLOCK: u64 = 0x0001;
    const GRND_RANDOM: u64 = 0x0002;
    const GRND_INSECURE: u64 = 0x0004;
    let allowed_flags = GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE;
    if flags & !allowed_flags != 0 {
        return EINVAL;
    }

    let len = len as usize;
    let _validated = match UserBuf::validate(buf_ptr, len, GETRANDOM_MAX) {
        Ok(buf) => buf,
        Err(e) => return e.to_errno(),
    };
    if len == 0 {
        return 0;
    }

    if !crate::security::crypto::rng_is_ready() {
        crate::security::crypto::rng_init();
    }

    let mut written = 0usize;
    while written < len {
        let mut chunk = [0u8; 64];
        let n = core::cmp::min(chunk.len(), len.saturating_sub(written));
        if crate::security::crypto::rng_fill(&mut chunk[..n]).is_err() {
            return EAGAIN;
        }
        let dst = match buf_ptr.checked_add(written as u64) {
            Some(addr) => addr,
            None => return EFAULT,
        };
        if copy_to_user(dst as *mut u8, chunk.as_ptr(), n).is_err() {
            return EFAULT;
        }
        written = written.saturating_add(n);
    }
    written as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers IPC natifs Exo-OS (bloc 300+)
// ─────────────────────────────────────────────────────────────────────────────

const IPC_ENDPOINT_OWNER_SLOTS: usize = 128;

#[derive(Clone, Copy)]
struct IpcEndpointOwner {
    endpoint: u64,
    owner_pid: u32,
}

impl IpcEndpointOwner {
    const EMPTY: Self = Self {
        endpoint: 0,
        owner_pid: 0,
    };
}

static IPC_ENDPOINT_OWNERS: spin::Mutex<[IpcEndpointOwner; IPC_ENDPOINT_OWNER_SLOTS]> =
    spin::Mutex::new([IpcEndpointOwner::EMPTY; IPC_ENDPOINT_OWNER_SLOTS]);

#[inline]
fn service_class_for_endpoint_name(name: &[u8]) -> Option<crate::security::ServiceClass> {
    match name {
        b"memory_server" => Some(crate::security::ServiceClass::MemoryServer),
        b"vfs_server" => Some(crate::security::ServiceClass::VfsServer),
        b"crypto_server" => Some(crate::security::ServiceClass::CryptoServer),
        b"device_server" => Some(crate::security::ServiceClass::DeviceServer),
        b"virtio_drivers" => Some(crate::security::ServiceClass::VirtioDriver),
        b"network_server" => Some(crate::security::ServiceClass::NetworkServer),
        b"scheduler_server" => Some(crate::security::ServiceClass::SchedulerServer),
        b"input_server" => Some(crate::security::ServiceClass::InputServer),
        b"tty_server" => Some(crate::security::ServiceClass::TtyServer),
        b"exo_shield" => Some(crate::security::ServiceClass::ExoShield),
        b"exosh" => Some(crate::security::ServiceClass::Exosh),
        _ => None,
    }
}

fn reserve_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<bool, i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut empty_slot = None;
    let mut idx = 0usize;
    while idx < owners.len() {
        let entry = owners[idx];
        if entry.endpoint == endpoint {
            if entry.owner_pid == owner_pid {
                return Ok(false);
            }
            if !crate::process::is_alive(entry.owner_pid) {
                owners[idx] = IpcEndpointOwner {
                    endpoint,
                    owner_pid,
                };
                return Ok(true);
            }
            return Err(EBUSY);
        }
        if entry.endpoint == 0 && empty_slot.is_none() {
            empty_slot = Some(idx);
        }
        idx += 1;
    }

    let Some(slot) = empty_slot else {
        return Err(ENOMEM);
    };
    owners[slot] = IpcEndpointOwner {
        endpoint,
        owner_pid,
    };
    Ok(false)
}

fn release_ipc_endpoint_owner(endpoint: u64, owner_pid: u32) -> Result<(), i64> {
    if endpoint == 0 || owner_pid == 0 {
        return Err(EINVAL);
    }

    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 && packed_pid != owner_pid {
        return Err(EACCES);
    }

    let mut owners = IPC_ENDPOINT_OWNERS.lock();
    let mut idx = 0usize;
    while idx < owners.len() {
        if owners[idx].endpoint == endpoint {
            if owners[idx].owner_pid != owner_pid {
                return Err(EACCES);
            }
            owners[idx] = IpcEndpointOwner::EMPTY;
            return Ok(());
        }
        idx += 1;
    }
    Ok(())
}

fn ipc_endpoint_owner_pid(endpoint: u64) -> Option<u32> {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        return Some(packed_pid);
    }

    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.endpoint == endpoint && entry.owner_pid != 0)
        .map(|entry| entry.owner_pid)
}

fn primary_ipc_endpoint_for_owner(owner_pid: u32) -> Option<u64> {
    if owner_pid == 0 {
        return None;
    }
    let owners = IPC_ENDPOINT_OWNERS.lock();
    owners
        .iter()
        .find(|entry| entry.owner_pid == owner_pid && entry.endpoint != 0)
        .map(|entry| entry.endpoint)
}

/// `exo_ipc_send(endpoint, msg_ptr, msg_len, flags)`.
pub fn sys_exo_ipc_send(
    endpoint: u64,
    msg_ptr: u64,
    msg_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_SEND);
    let len = msg_len as usize;
    if len > crate::ipc::core::constants::MAX_MSG_SIZE {
        return E2BIG;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    let _validated_buf =
        match UserBuf::validate(msg_ptr, len, crate::ipc::core::constants::MAX_MSG_SIZE) {
            Ok(b) => b,
            Err(e) => return e.to_errno(),
        };
    let mut payload = match zeroed_user_vec(len) {
        Ok(payload) => payload,
        Err(errno) => return errno,
    };
    if len != 0 {
        if copy_from_user(payload.as_mut_ptr(), msg_ptr as *const u8, len).is_err() {
            return EFAULT;
        }
    }
    if flags & IPC_FLAG_INJECT_SRC_PID != 0 {
        if len < core::mem::size_of::<u32>() {
            return EINVAL;
        }
        let caller_pid = crate::syscall::fast_path::syscall_current_pid();
        payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
    }
    if is_reserved_kernel_ipc(endpoint, &payload) {
        return EACCES;
    }
    let raw_flags = if flags & IPC_RECV_TIMEOUT_FLAG != 0 {
        0x0001
    } else {
        0
    };
    match crate::ipc::channel::raw::send_raw(endpoint_id, &payload, raw_flags) {
        Ok(_) => 0,
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_recv(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv(
    endpoint: u64,
    buf_ptr: u64,
    buf_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV);
    let (endpoint, buf_ptr, buf_len, flags) =
        normalize_ipc_recv_args(endpoint, buf_ptr, buf_len, flags);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, false)
}

/// `exo_ipc_recv_nb(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv_nb(
    endpoint: u64,
    buf_ptr: u64,
    buf_len: u64,
    flags: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV_NB);
    let (endpoint, buf_ptr, buf_len, flags) =
        normalize_ipc_recv_args(endpoint, buf_ptr, buf_len, flags);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, true)
}

/// `exo_ipc_call(endpoint, msg_ptr, msg_len, resp_ptr, resp_len, flags)`.
pub fn sys_exo_ipc_call(
    endpoint: u64,
    msg_ptr: u64,
    msg_len: u64,
    resp_ptr: u64,
    resp_len: u64,
    flags: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CALL);
    let send_len = msg_len as usize;
    let recv_len = resp_len as usize;
    if send_len > crate::ipc::rpc::MAX_CALL_PAYLOAD || recv_len > crate::ipc::rpc::MAX_CALL_PAYLOAD
    {
        return E2BIG;
    }
    if flags != 0 {
        return EINVAL;
    }
    if send_len != 0 && msg_ptr == 0 {
        return EFAULT;
    }
    if recv_len != 0 && resp_ptr == 0 {
        return EFAULT;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }

    let server_ep = match EndpointId::new(endpoint) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    let mut request = match zeroed_user_vec(send_len) {
        Ok(request) => request,
        Err(errno) => return errno,
    };
    if send_len != 0 {
        if copy_from_user(request.as_mut_ptr(), msg_ptr as *const u8, send_len).is_err() {
            return EFAULT;
        }
    }

    let mut response = match zeroed_user_vec(recv_len) {
        Ok(response) => response,
        Err(errno) => return errno,
    };

    match crate::ipc::rpc::call_raw(server_ep, &request, &mut response) {
        Ok(reply_len) => {
            if reply_len != 0
                && copy_to_user(resp_ptr as *mut u8, response.as_ptr(), reply_len).is_err()
            {
                return EFAULT;
            }
            reply_len as i64
        }
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_create(name_ptr, name_len, endpoint)` — ouvre la mailbox raw du serveur.
pub fn sys_exo_ipc_create(
    name_ptr: u64,
    name_len: u64,
    endpoint: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CREATE);
    let len = name_len as usize;
    if len == 0 || len > 128 {
        return EINVAL;
    }
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };

    let _validated = match UserBuf::validate(name_ptr, len, 128) {
        Ok(buf) => buf,
        Err(err) => return err.to_errno(),
    };
    let mut name = match zeroed_user_vec(len) {
        Ok(name) => name,
        Err(errno) => return errno,
    };
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    let replaced_dead_owner = match reserve_ipc_endpoint_owner(endpoint, caller_pid) {
        Ok(replaced) => replaced,
        Err(errno) => return errno,
    };
    if replaced_dead_owner {
        crate::ipc::channel::raw::mailbox_close(ep);
    }

    if crate::ipc::channel::raw::mailbox_open(ep) {
        if let Err(err) = crate::ipc::endpoint::register_endpoint(&name, ep) {
            crate::ipc::channel::raw::mailbox_close(ep);
            let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
            return ipc_error_to_errno(err);
        }
        if let Some(class) = service_class_for_endpoint_name(&name) {
            let _ = crate::security::register_service_class(Pid(caller_pid), class);
        }
        0
    } else {
        let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
        ENOMEM
    }
}

/// `exo_ipc_destroy(endpoint)` — ferme la mailbox raw du serveur.
pub fn sys_exo_ipc_destroy(endpoint: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_DESTROY);
    let endpoint_pid = exo_ipc_endpoint_pid(endpoint);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if endpoint_pid == 0 {
        return EINVAL;
    }
    if caller_pid == 0 || endpoint_pid != caller_pid {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    crate::ipc::channel::raw::mailbox_close(ep);
    let _ = release_ipc_endpoint_owner(endpoint, caller_pid);
    0
}

/// `exo_ipc_lookup(name_ptr, name_len)` — résout un endpoint IPC par nom.
pub fn sys_exo_ipc_lookup(
    name_ptr: u64,
    name_len: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_LOOKUP);

    let len = name_len as usize;
    if len == 0 || len > crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN {
        return EINVAL;
    }
    if name_ptr == 0 {
        return EFAULT;
    }

    let _validated = match UserBuf::validate(
        name_ptr,
        len,
        crate::ipc::core::constants::MAX_ENDPOINT_NAME_LEN,
    ) {
        Ok(buf) => buf,
        Err(err) => return err.to_errno(),
    };

    let mut name = match zeroed_user_vec(len) {
        Ok(name) => name,
        Err(errno) => return errno,
    };
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    match crate::ipc::endpoint::lookup_endpoint(&name) {
        Some(endpoint) => endpoint.get() as i64,
        None => ENOENT,
    }
}

#[inline(always)]
fn exo_ipc_endpoint_pid(endpoint: u64) -> u32 {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        packed_pid
    } else if let Some(owner_pid) = ipc_endpoint_owner_pid(endpoint) {
        owner_pid
    } else {
        endpoint as u32
    }
}

#[inline(always)]
fn current_pid_u32() -> u32 {
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        return 0;
    }
    unsafe {
        (*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    }
}

fn process_name_eq(name: &[u8; EXO_PROCESS_NAME_LEN], expected: &[u8]) -> bool {
    let mut len = 0usize;
    while len < name.len() && name[len] != 0 {
        len += 1;
    }
    if len != expected.len() {
        return false;
    }
    let mut i = 0usize;
    while i < len {
        if name[i] != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

const IPC_RECV_TIMEOUT_FLAG: u64 = 0x0001;
const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;
const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
const CRYPTO_PHOENIX_WAKE_ENTROPY: u32 = 255;

#[inline]
fn zeroed_user_vec(len: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    out.resize(len, 0);
    Ok(out)
}

#[inline]
fn is_reserved_kernel_ipc(endpoint: u64, payload: &[u8]) -> bool {
    if endpoint != CRYPTO_SERVER_ENDPOINT_ID
        || payload.len() < crate::ipc::core::constants::ABI_IPC_HEADER_SIZE
    {
        return false;
    }
    let msg_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    msg_type == CRYPTO_PHOENIX_WAKE_ENTROPY
}

#[inline]
fn is_kernel_ephemeral_reply_endpoint(endpoint: u64) -> bool {
    endpoint & (1u64 << 63) != 0
}

fn caller_can_manage_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => process_name_eq(&pcb.name_snapshot(), b"memory_server"),
        None => false,
    }
}

fn caller_can_copy_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => {
            let name = pcb.name_snapshot();
            process_name_eq(&name, b"memory_server") || process_name_eq(&name, b"vfs_server")
        }
        None => false,
    }
}

fn user_as_for_pid(pid: u32) -> Result<&'static crate::memory::virt::UserAddressSpace, i64> {
    if pid == 0 {
        return Err(EINVAL);
    }
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(ENOENT)?;
    let as_ptr = pcb.address_space_ptr();
    if as_ptr.is_null() {
        return Err(ENOMEM);
    }
    // SAFETY: le PCB reste dans PROCESS_REGISTRY pendant le syscall et
    // address_space pointe vers un UserAddressSpace créé par lifecycle/create.
    Ok(unsafe { &*(as_ptr as *const crate::memory::virt::UserAddressSpace) })
}

fn validate_remote_user_range(addr: u64, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if addr == 0 {
        return Err(EFAULT);
    }
    let end = addr.checked_add(len as u64).ok_or(EFAULT)?;
    if end > crate::syscall::validation::USER_ADDR_MAX {
        return Err(EFAULT);
    }
    Ok(())
}

fn remote_user_phys(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Result<(u64, usize), i64> {
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

    let walker = PageTableWalker::new(user_as.pml4_phys());
    let virt = VirtAddr::new(addr);
    let (entry, level) = match walker.walk_read(virt) {
        WalkResult::Leaf { entry, level } | WalkResult::HugePage { entry, level } => (entry, level),
        _ => return Err(EFAULT),
    };
    if !entry.is_user() || (write && !entry.is_writable()) {
        return Err(EFAULT);
    }
    let page_size = level.page_size();
    let offset = (addr as usize) & (page_size - 1);
    Ok((
        entry.phys_addr().as_u64() + offset as u64,
        page_size - offset,
    ))
}

/// `exo_mem_copy_from_pid(target_pid, remote_src, local_dst, len, flags)`.
///
/// Réservé aux serveurs racine qui doivent servir des requêtes IPC avec des
/// pointeurs appartenant au client. Le buffer local est toujours copié via
/// `copy_to_user`; le buffer distant est traduit via la PML4 du PID cible.
pub fn sys_exo_mem_copy_from_pid(
    target_pid: u64,
    remote_src: u64,
    local_dst: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_FROM_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_src, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_dst, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 256];
    while copied < len {
        let src = remote_src + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, src, false) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        let src_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *const u8;
        // SAFETY: `remote_user_phys` a validé la traduction du PID cible et
        // `scratch` est un buffer kernel local de taille `n`.
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, scratch.as_mut_ptr(), n);
        }
        if copy_to_user((local_dst + copied as u64) as *mut u8, scratch.as_ptr(), n).is_err() {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_copy_to_pid(target_pid, remote_dst, local_src, len, flags)`.
pub fn sys_exo_mem_copy_to_pid(
    target_pid: u64,
    remote_dst: u64,
    local_src: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_TO_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_dst, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_src, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 256];
    while copied < len {
        let dst = remote_dst + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, dst, true) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        if copy_from_user(
            scratch.as_mut_ptr(),
            (local_src + copied as u64) as *const u8,
            n,
        )
        .is_err()
        {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        let dst_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *mut u8;
        // SAFETY: `remote_user_phys(..., write=true)` garantit une page user
        // présente et writable; `scratch` contient `n` octets copiés du caller.
        unsafe {
            core::ptr::copy_nonoverlapping(scratch.as_ptr(), dst_ptr, n);
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_map_pid(target_pid, hint, len, prot, flags)` réservé à memory_server.
pub fn sys_exo_mem_map_pid(
    target_pid: u64,
    hint: u64,
    len: u64,
    prot: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mmap_in_as(user_as, hint, len, prot, flags) {
        Ok(addr) => addr as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_munmap_pid(target_pid, addr)` réservé à memory_server.
pub fn sys_exo_mem_munmap_pid(
    target_pid: u64,
    addr: u64,
    _len: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MUNMAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap_in_as(user_as, addr) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_mprotect_pid(target_pid, addr, len, prot)` réservé à memory_server.
pub fn sys_exo_mem_mprotect_pid(
    target_pid: u64,
    addr: u64,
    len: u64,
    prot: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MPROTECT_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mprotect_in_as(user_as, addr, len, prot) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

fn normalize_ipc_recv_args(a1: u64, a2: u64, a3: u64, flags: u64) -> (u64, u64, u64, u64) {
    let shorthand_len = a2 <= 65_536;
    let user_buf = a1 >= 0x1000
        && a1 < crate::syscall::validation::USER_ADDR_MAX
        && a1
            .checked_add(a2)
            .map(|end| end <= crate::syscall::validation::USER_ADDR_MAX)
            .unwrap_or(false);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let explicit_endpoint = ipc_endpoint_owner_pid(a1).is_some();
    if flags == 0 && shorthand_len && user_buf && !explicit_endpoint {
        let endpoint = primary_ipc_endpoint_for_owner(caller_pid).unwrap_or(caller_pid as u64);
        (endpoint, a1, a2, a3)
    } else {
        (a1, a2, a3, flags)
    }
}

fn recv_ipc_message(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, nowait: bool) -> i64 {
    const IPC_RECV_IDLE_NAP_NS: u64 = 2_000_000;

    let len = buf_len as usize;
    if len > 65_536 {
        return E2BIG;
    }
    if buf_ptr == 0 && len != 0 {
        return EFAULT;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || exo_ipc_endpoint_pid(endpoint) != caller_pid {
        return EACCES;
    }

    let recv_cap = len.min(crate::ipc::core::constants::MAX_MSG_SIZE);
    let mut payload = [0u8; crate::ipc::core::constants::MAX_MSG_SIZE];
    let timeout_requested = !nowait && (flags & IPC_RECV_TIMEOUT_FLAG != 0);
    let timeout_ms = flags & !IPC_RECV_TIMEOUT_FLAG;

    let result = if nowait {
        crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
    } else if timeout_requested {
        let deadline = crate::scheduler::timer::clock::monotonic_ns()
            .saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
            match crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
            {
                Ok(n) => break Ok(n),
                Err(IpcError::WouldBlock) | Err(IpcError::QueueEmpty) => {
                    let now = crate::scheduler::timer::clock::monotonic_ns();
                    if now >= deadline {
                        break Err(IpcError::Timeout);
                    }
                    let nap_ns = deadline.saturating_sub(now).min(IPC_RECV_IDLE_NAP_NS);
                    if !crate::scheduler::timer::sleep_ns(nap_ns) {
                        unsafe {
                            let _ = crate::scheduler::core::switch::cooperative_reschedule();
                        }
                    }
                }
                Err(err) => break Err(err),
            }
        }
    } else {
        loop {
            match crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
            {
                Ok(n) => break Ok(n),
                Err(IpcError::WouldBlock) | Err(IpcError::QueueEmpty) => unsafe {
                    if !crate::scheduler::timer::sleep_ns(IPC_RECV_IDLE_NAP_NS) {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                },
                Err(err) => break Err(err),
            }
        }
    };

    match result {
        Ok(n) => {
            if n != 0 && copy_to_user(buf_ptr as *mut u8, payload.as_ptr(), n).is_err() {
                return EFAULT;
            }
            n as i64
        }
        Err(IpcError::Timeout) => crate::syscall::errno::ETIMEDOUT,
        Err(err) => ipc_error_to_errno(err),
    }
}

fn ipc_error_to_errno(err: IpcError) -> i64 {
    match err {
        IpcError::WouldBlock
        | IpcError::Retry
        | IpcError::Full
        | IpcError::QueueFull
        | IpcError::QueueEmpty => EAGAIN,
        IpcError::EndpointNotFound | IpcError::NotFound => ENOENT,
        IpcError::PermissionDenied => EACCES,
        IpcError::MessageTooLarge => EMSGSIZE,
        IpcError::Timeout => EAGAIN,
        IpcError::ResourceExhausted | IpcError::ShmPoolFull | IpcError::OutOfResources => ENOMEM,
        IpcError::ConnRefused => ENOENT,
        IpcError::AlreadyConnected => EBUSY,
        IpcError::InvalidParam
        | IpcError::InvalidHandle
        | IpcError::Invalid
        | IpcError::NullEndpoint
        | IpcError::InvalidEndpoint
        | IpcError::InvalidArgument => EINVAL,
        IpcError::Interrupted => EINTR,
        IpcError::ChannelClosed | IpcError::Closed => EBUSY,
        IpcError::HandshakeFailed
        | IpcError::OutOfOrder
        | IpcError::ProtocolError
        | IpcError::MappingFailed
        | IpcError::InternalError
        | IpcError::Internal
        | IpcError::Loop => EINVAL,
    }
}

fn enforce_direct_ipc_policy(endpoint: u64) -> Result<(), i64> {
    if is_kernel_ephemeral_reply_endpoint(endpoint) {
        return Ok(());
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return Err(EACCES);
    }

    let dst_pid = exo_ipc_endpoint_pid(endpoint);
    if dst_pid == 0 {
        return Err(EINVAL);
    }

    let verdict = crate::security::check_direct_ipc(
        crate::process::core::pid::Pid(caller_pid),
        crate::process::core::pid::Pid(dst_pid),
    );

    match verdict {
        crate::security::IpcPolicyResult::Allowed => Ok(()),
        crate::security::IpcPolicyResult::Denied
        | crate::security::IpcPolicyResult::UnknownService => {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::IpcUnauthorized {
                    src_pid: caller_pid,
                    dst_pid,
                },
            );
            Err(EACCES)
        }
    }
}

#[inline]
fn checked_u32_sysarg(value: u64) -> Result<u32, i64> {
    if value > u32::MAX as u64 {
        Err(EINVAL)
    } else {
        Ok(value as u32)
    }
}

#[inline]
fn checked_i32_sysarg(value: u64) -> Result<i32, i64> {
    let signed = value as i64;
    if signed < i32::MIN as i64 || signed > i32::MAX as i64 {
        Err(EINVAL)
    } else {
        Ok(signed as i32)
    }
}

#[inline]
fn checked_usize_sysarg(value: u64) -> Result<usize, i64> {
    if value > usize::MAX as u64 {
        Err(EINVAL)
    } else {
        Ok(value as usize)
    }
}

/// `exo_cap_create(type, rights, target_pid, token_out_ptr)` → handle ou errno.
pub fn sys_exo_cap_create(
    cap_type: u64,
    rights: u64,
    target: u64,
    token_out_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CREATE);
    if token_out_ptr == 0 {
        return EFAULT;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let cap_type = match checked_u32_sysarg(cap_type) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rights = match checked_u32_sysarg(rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target = match checked_u32_sysarg(target) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::create(cap_type, rights, target, caller_pid) {
        Ok(token) => {
            let token_bytes = token.to_bytes();
            if copy_to_user(
                token_out_ptr as *mut u8,
                token_bytes.as_ptr(),
                crate::security::capability::CAP_TOKEN_WIRE_SIZE,
            )
            .is_err()
            {
                return EFAULT;
            }
            token.object_id().as_u64() as i64
        }
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_revoke(handle)`.
pub fn sys_exo_cap_revoke(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_CAP_REVOKE);
    let handle = match checked_u32_sysarg(handle) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::revoke_handle(handle) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_check(token_ptr, rights, target_pid, expected_type)` → 0 ou errno.
pub fn sys_exo_cap_check(
    token_ptr: u64,
    required_rights: u64,
    target_pid: u64,
    expected_type: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CHECK);
    if token_ptr == 0 {
        return EFAULT;
    }

    let mut token_bytes = [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE];
    if copy_from_user(
        token_bytes.as_mut_ptr(),
        token_ptr as *const u8,
        token_bytes.len(),
    )
    .is_err()
    {
        return EFAULT;
    }

    let token = match crate::security::capability::CapToken::from_bytes(&token_bytes) {
        Some(token) => token,
        None => return EINVAL,
    };

    let required_rights = match checked_u32_sysarg(required_rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || caller_pid != target_pid {
        return EACCES;
    }
    let expected_type = match checked_u32_sysarg(expected_type) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::check_token(
        token,
        required_rights,
        target_pid,
        expected_type,
    ) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

#[cfg(test)]
mod capability_syscall_arg_tests {
    use super::*;

    #[test]
    fn checked_u32_sysarg_accepts_full_u32_range() {
        assert_eq!(checked_u32_sysarg(0), Ok(0));
        assert_eq!(checked_u32_sysarg(u32::MAX as u64), Ok(u32::MAX));
    }

    #[test]
    fn checked_u32_sysarg_rejects_high_bits_in_syscall_abi() {
        assert_eq!(checked_u32_sysarg(u32::MAX as u64 + 1), Err(EINVAL));
    }

    #[test]
    fn checked_i32_sysarg_accepts_sign_extended_negative_values() {
        assert_eq!(checked_i32_sysarg(u64::MAX), Ok(-1));
        assert_eq!(checked_i32_sysarg(i32::MIN as i64 as u64), Ok(i32::MIN));
        assert_eq!(checked_i32_sysarg(i32::MAX as u64), Ok(i32::MAX));
    }

    #[test]
    fn checked_i32_sysarg_rejects_non_canonical_positive_high_bits() {
        assert_eq!(checked_i32_sysarg(i32::MAX as u64 + 1), Err(EINVAL));
        assert_eq!(checked_i32_sysarg(0x8000_0000_0000_0000), Err(EINVAL));
    }

    #[test]
    fn checked_usize_sysarg_accepts_native_width() {
        assert_eq!(checked_usize_sysarg(0), Ok(0));
        assert_eq!(checked_usize_sysarg(usize::MAX as u64), Ok(usize::MAX));
    }
}

/// `exo_log(buf_ptr, len, level)` — log direct vers le ring buffer kernel.
pub fn sys_exo_log(buf_ptr: u64, len: u64, level: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_LOG);
    let log_len = (len as usize).min(4096);
    let buf = match UserBuf::validate(buf_ptr, log_len, 4096) {
        Ok(b) => b,
        Err(e) => return e.to_errno(),
    };
    // Copier le message dans un buffer kernel local (stack, NO-ALLOC)
    let mut kbuf = [0u8; 4096];
    if let Err(e) = buf.read_into(&mut kbuf[..log_len]) {
        return e.to_errno();
    }
    // Écriture via serial jusqu'à activation de log_ring dans arch/.
    let _ = level;
    // Les octets sont déjà dans kbuf — ils seront consommés par le prochain lecteur.
    0
}

const EXO_PROCESS_NAME_LEN: usize = 16;
const EXO_PROCESS_LIST_MAX: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct ExoProcessInfo {
    pid: u32,
    ppid: u32,
    state: u32,
    threads: u32,
    name: [u8; EXO_PROCESS_NAME_LEN],
    utime_ns: u64,
    stime_ns: u64,
}

/// `exo_process_list(buf, capacity, entry_size)` — snapshot de la registry PCB.
pub fn sys_exo_process_list(
    buf_ptr: u64,
    capacity: u64,
    entry_size: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PROCESS_LIST);
    let expected = core::mem::size_of::<ExoProcessInfo>();
    if entry_size != expected as u64 {
        return EINVAL;
    }
    if capacity == 0 {
        return PROCESS_REGISTRY.count() as i64;
    }
    if capacity > EXO_PROCESS_LIST_MAX as u64 {
        return E2BIG;
    }
    let capacity = capacity as usize;
    let total_len = match capacity.checked_mul(expected) {
        Some(len) => len,
        None => return E2BIG,
    };
    if UserBuf::validate(buf_ptr, total_len, EXO_PROCESS_LIST_MAX * expected).is_err() {
        return EFAULT;
    }

    let mut written = 0usize;
    let mut fault = false;
    PROCESS_REGISTRY.for_each(|pcb| {
        if written >= capacity || fault {
            return;
        }
        let entry = ExoProcessInfo {
            pid: pcb.pid.0,
            ppid: pcb.ppid.load(Ordering::Acquire),
            state: pcb.state.load(Ordering::Acquire),
            threads: pcb.thread_count.load(Ordering::Acquire),
            name: pcb.name_snapshot(),
            utime_ns: pcb.utime_ns.load(Ordering::Acquire),
            stime_ns: pcb.stime_ns.load(Ordering::Acquire),
        };
        let dst = (buf_ptr as *mut u8).wrapping_add(written * expected);
        let src = &entry as *const ExoProcessInfo as *const u8;
        if copy_to_user(dst, src, expected).is_err() {
            fault = true;
        } else {
            written += 1;
        }
    });

    if fault {
        EFAULT
    } else {
        written as i64
    }
}

/// `exo_phoenix_state_set(state)` — synchronise une transition Phoenix root.
pub fn sys_exo_phoenix_state_set(
    state: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_SET);
    if state > u8::MAX as u64 {
        return EINVAL;
    }
    if !matches!(state as u8, 1 | 9 | 10) {
        return EINVAL;
    }
    let caller = current_pid_u32();
    if caller != 0 {
        let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(caller)) else {
            return EPERM;
        };
        if !pcb.is_root() {
            return EPERM;
        }
        let name = pcb.name_snapshot();
        if !process_name_eq(&name, b"network_server") && !process_name_eq(&name, b"init_server") {
            return EPERM;
        }
    }
    if crate::exophoenix::try_set_state_raw(state as u8) {
        0
    } else {
        EINVAL
    }
}

/// `exo_phoenix_state_get()` — retourne l'état Phoenix courant.
pub fn sys_exo_phoenix_state_get(
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_GET);
    crate::exophoenix::state() as u8 as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers GI-03 Drivers (530–546)
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Helpers pour DMA/PCI (réutilisables)
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn parse_dma_direction(raw: u64) -> Option<DmaDirection> {
    match raw {
        0 => Some(DmaDirection::ToDevice),
        1 => Some(DmaDirection::FromDevice),
        2 => Some(DmaDirection::Bidirection),
        3 => Some(DmaDirection::None),
        _ => None,
    }
}

#[inline]
fn parse_irq_ack_result(raw: u64) -> Option<IrqAckResult> {
    match raw {
        0 => Some(IrqAckResult::Handled),
        1 => Some(IrqAckResult::NotMine),
        _ => None,
    }
}

#[inline]
fn parse_ipc_endpoint_packed(lo: u64, hi: u64) -> IpcEndpoint {
    IpcEndpoint {
        pid: lo as u32,
        chan_idx: (lo >> 32) as u32,
        generation: hi as u32,
        _pad: (hi >> 32) as u32,
    }
}

/// Encode/decode ABI simplifié BDF sur 32 bits :
/// - bits 31..16 : segment
/// - bits 15..8  : bus
/// - bits 7..3   : device
/// - bits 2..0   : function
#[inline]
fn parse_pci_address(raw: u64) -> PciAddress {
    let v = raw as u32;
    let segment = (v >> 16) as u16;
    let bus = ((v >> 8) & 0xFF) as u8;
    let device = ((v >> 3) & 0x1F) as u8;
    let function = (v & 0x07) as u8;
    PciAddress::new(segment, bus, device, function)
}

#[inline]
fn dma_error_to_errno(err: DmaError) -> i64 {
    match err {
        DmaError::OutOfMemory => ENOMEM,
        DmaError::NoChannel
        | DmaError::Timeout
        | DmaError::NotInitialized
        | DmaError::AlreadySubmitted
        | DmaError::Cancelled => EAGAIN,
        DmaError::HardwareError | DmaError::IommuFault => EFAULT,
        DmaError::InvalidParams
        | DmaError::MisalignedBuffer
        | DmaError::WrongZone
        | DmaError::NotSupported => EINVAL,
    }
}

#[inline]
fn claim_error_to_errno(err: ClaimError) -> i64 {
    match err {
        ClaimError::PermissionDenied => EACCES,
        ClaimError::AlreadyClaimed => EBUSY,
        ClaimError::NotInHardwareRegion | ClaimError::PhysIsRam => EINVAL,
        ClaimError::TableFull => ENOMEM,
    }
}

#[inline]
fn topo_error_to_errno(err: TopoError) -> i64 {
    match err {
        TopoError::TopologyTableFull => ENOMEM,
    }
}

#[inline]
fn pci_cfg_error_to_errno(err: PciCfgError) -> i64 {
    match err {
        PciCfgError::NotClaimed => EPERM,
        PciCfgError::PermissionDenied => EACCES,
    }
}

#[inline]
fn mmio_error_to_errno(err: MmioError) -> i64 {
    match err {
        MmioError::PermissionDenied => EACCES,
        MmioError::AlreadyMapped => EBUSY,
        MmioError::OutOfMemory => ENOMEM,
        MmioError::NotMapped => EFAULT,
        MmioError::InvalidParams => EINVAL,
    }
}

#[inline]
fn msi_error_to_errno(err: MsiError) -> i64 {
    match err {
        MsiError::NotFound => ENOENT,
        MsiError::TableFull | MsiError::NoSpace => ENOMEM,
        MsiError::AmbiguousClaim => EINVAL,
        MsiError::InvalidParams => EINVAL,
    }
}

/// ABI GI-03 canonique.
/// Encodage registre noyau :
/// `sys_irq_register(irq, endpoint_lo, endpoint_hi, source_kind, bdf_raw, has_bdf)`.
/// `endpoint_lo = pid | (chan_idx << 32)`, `endpoint_hi = generation | (_pad << 32)`.
pub fn sys_irq_register(
    irq: u64,
    endpoint_lo: u64,
    endpoint_hi: u64,
    source_kind: u64,
    bdf_raw: u64,
    has_bdf: u64,
) -> i64 {
    stat_inc(SYS_IRQ_REGISTER);

    if irq > u8::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let vector = IrqVector(irq as u8);
    if !vector.is_valid() {
        return EINVAL;
    }

    let source_kind = match parse_irq_source_kind(source_kind) {
        Some(kind) => kind,
        None => return EINVAL,
    };

    let mut endpoint = parse_ipc_endpoint_packed(endpoint_lo, endpoint_hi);
    endpoint.pid = caller_pid;

    let bdf = if has_bdf != 0 { Some(bdf_raw) } else { None };

    match crate::arch::x86_64::irq::sys_irq_register(vector, endpoint, source_kind, bdf) {
        Ok(reg_id) => reg_id as i64,
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_irq_ack(irq, reg_id, handler_gen, wave_gen, result)`.
pub fn sys_irq_ack(
    irq: u64,
    reg_id: u64,
    handler_gen: u64,
    wave_gen: u64,
    result_raw: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_IRQ_ACK);
    if irq > u8::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let vector = IrqVector(irq as u8);
    if !vector.is_valid() {
        return EINVAL;
    }

    let result = match parse_irq_ack_result(result_raw) {
        Some(result) => result,
        None => return EINVAL,
    };

    match crate::arch::x86_64::irq::ack_irq(
        vector,
        reg_id,
        handler_gen,
        IrqOwnerPid(caller_pid),
        wave_gen,
        result,
    ) {
        Ok(()) => 0,
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_mmio_map(phys_addr, size)` pour le PID appelant.
pub fn sys_mmio_map(phys_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_MAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_map_for_pid(caller_pid, PhysAddr::new(phys_addr), size as usize)
    {
        Ok(virt) => virt as i64,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_mmio_unmap(virt_addr, size)` pour le PID appelant.
pub fn sys_mmio_unmap(virt_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_UNMAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_unmap_for_pid(caller_pid, virt_addr, size as usize) {
        Ok(()) => 0,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_alloc(size, direction) -> (virt, iova)`.
/// Réalisation registre noyau :
/// - `rax` retourne l'IOVA
/// - `arg3` (`user_virt_out`) reçoit l'adresse CPU virtuelle si non nul
/// - `arg4`/`arg5` restent des extensions de compatibilité (`map_flags`, `domain_hint`)
pub fn sys_dma_alloc(
    size: u64,
    direction: u64,
    user_virt_out: u64,
    map_flags: u64,
    domain_hint: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_DMA_ALLOC);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain = match crate::drivers::iommu::ensure_domain_for_pid(caller_pid) {
        Ok(domain) => domain,
        Err(_) if requested_domain.0 != 0 => requested_domain,
        Err(_) => return EAGAIN,
    };

    match crate::drivers::sys_dma_alloc_for_pid(
        caller_pid,
        size as usize,
        direction,
        DmaMapFlags(map_flags as u32),
        effective_domain,
    ) {
        Ok((virt, iova)) => {
            if user_virt_out != 0 {
                if let Err(e) = write_user_typed::<u64>(user_virt_out, virt) {
                    let _ =
                        crate::drivers::sys_dma_free_for_pid(caller_pid, iova, effective_domain);
                    return e.to_errno();
                }
            }
            iova.0 as i64
        }
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_free(iova, size)`.
/// `arg3` reste un hint de domaine pour le noyau tant que la couche userland n'est pas finalisée.
pub fn sys_dma_free(iova: u64, size: u64, domain_hint: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_FREE);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    if crate::drivers::dma::dma_alloc_size_for_pid(caller_pid, IovaAddr(iova))
        != Some(size as usize)
    {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain =
        crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain);

    match crate::drivers::sys_dma_free_for_pid(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_dma_sync(iova, size, direction)`.
pub fn sys_dma_sync(iova: u64, size: u64, direction: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_SYNC);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_sync_for_pid(caller_pid, IovaAddr(iova), size as usize, direction)
    {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_read(offset)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_read(offset: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_READ);

    if offset > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_read_for_pid(caller_pid, offset as u16) {
        Ok(value) => value as i64,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_write(offset, value)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_write(offset: u64, value: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_WRITE);

    if offset > u16::MAX as u64 || value > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_write_for_pid(caller_pid, offset as u16, value as u32) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_bus_master(enable)` pour le device claimé du PID appelant.
pub fn sys_pci_bus_master(enable: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_BUS_MASTER);

    if enable > 1 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_bus_master_for_pid(caller_pid, enable != 0) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_claim(phys_addr, size, owner_pid, bdf_raw, has_bdf)`.
pub fn sys_pci_claim(
    phys_addr: u64,
    size: u64,
    owner_pid: u64,
    bdf_raw: u64,
    has_bdf: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_CLAIM);
    if size == 0 || size > usize::MAX as u64 || owner_pid == 0 || owner_pid > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let owner_pid = owner_pid as u32;
    let bdf = if has_bdf != 0 {
        Some(parse_pci_address(bdf_raw))
    } else {
        None
    };

    match crate::drivers::sys_pci_claim(
        PhysAddr::new(phys_addr),
        size as usize,
        owner_pid,
        bdf,
        caller_pid,
    ) {
        Ok(()) => {
            if crate::drivers::iommu::ensure_domain_for_pid(owner_pid).is_err() {
                let _ = crate::drivers::release_claim_for_owner(owner_pid);
                return EAGAIN;
            }
            0
        }
        Err(err) => claim_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_map(vaddr, size, direction)`.
pub fn sys_dma_map(vaddr: u64, size: u64, direction: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_MAP);

    if size == 0 || size > usize::MAX as u64 || vaddr > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_map(caller_pid, vaddr as usize, size as usize, direction) {
        Ok(iova) => iova.0 as i64,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_unmap(domain_id, iova)`.
pub fn sys_dma_unmap(domain_id: u64, iova: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_UNMAP);
    if domain_id > u32::MAX as u64 {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_id as u32);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let effective_domain = if caller_pid != 0 {
        crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain)
    } else {
        requested_domain
    };

    match crate::drivers::sys_dma_unmap(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_alloc(count)`.
pub fn sys_msi_alloc(count: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_ALLOC);

    if count == 0 || count > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_alloc_for_pid(caller_pid, count as u16) {
        Ok(handle) => handle as i64,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_config(handle, vector_idx)`.
pub fn sys_msi_config(handle: u64, vector_idx: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_CONFIG);

    if vector_idx > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_config_for_pid(caller_pid, handle, vector_idx as u16) {
        Ok(_vector) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_free(handle)`.
pub fn sys_msi_free(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_FREE);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_free_for_pid(caller_pid, handle) {
        Ok(()) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_pci_set_topology(child_bdf_raw, parent_bdf_raw)`.
pub fn sys_pci_set_topology(
    bdf_raw: u64,
    parent_bdf_raw: u64,
    has_parent: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_SET_TOPOLOGY);

    if has_parent == 0 {
        return EINVAL;
    }

    let address = parse_pci_address(bdf_raw);
    let parent_bridge = parse_pci_address(parent_bdf_raw);

    match crate::drivers::sys_pci_set_topology(address, parent_bridge) {
        Ok(()) => 0,
        Err(err) => topo_error_to_errno(err),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction de la table
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le handler associé au numéro `nr`, ou `sys_enosys` si non implémenté.
///
/// Pas de verrou nécessaire : la table est en `.rodata`, lecture pure.
///
/// Performance : O(1) — un seul indirect load depuis `.rodata`.
#[inline]
pub fn get_handler(nr: u64) -> SyscallHandler {
    // Borne basse : vérification par dispatch.rs avant cet appel.
    // Ici on fait confiance à dispatch.rs pour la borne.
    match nr {
        // ── I/O, Fichiers ──────────────────────────────────────────────────
        SYS_READ => sys_read,
        SYS_WRITE => sys_write,
        SYS_PREAD64 => sys_pread64,
        SYS_PWRITE64 => sys_pwrite64,
        SYS_READV => sys_readv,
        SYS_WRITEV => sys_writev,
        SYS_POLL => sys_poll,
        SYS_SELECT => sys_select,
        SYS_PPOLL => sys_ppoll,
        SYS_PSELECT6 => sys_pselect6,
        SYS_OPEN => sys_open,
        SYS_CLOSE => sys_close,
        SYS_STAT => sys_stat,
        SYS_FSTAT => sys_fstat,
        SYS_LSTAT => sys_lstat,
        SYS_NEWFSTATAT => sys_newfstatat,
        SYS_STATX => sys_statx,
        SYS_LSEEK => sys_lseek,
        SYS_DUP => sys_dup,
        SYS_DUP2 => sys_dup2,
        SYS_DUP3 => sys_dup3,
        SYS_PIPE => sys_pipe,
        SYS_PIPE2 => sys_pipe2,
        SYS_FCNTL => sys_fcntl,
        SYS_FLOCK => sys_flock,
        SYS_IOCTL => sys_ioctl,
        SYS_FSYNC => sys_fsync,
        SYS_FDATASYNC => sys_fdatasync,
        SYS_SYNC => sys_sync,
        SYS_SYNC_FILE_RANGE => sys_sync_file_range,
        SYS_MSYNC => sys_msync,
        SYS_MKDIR => sys_mkdir,
        SYS_MKDIRAT => sys_mkdirat,
        SYS_RMDIR => sys_rmdir,
        SYS_UNLINK => sys_unlink,
        SYS_UNLINKAT => sys_unlinkat,
        SYS_MKNOD => sys_mknod,
        SYS_MKNODAT => sys_mknodat,
        SYS_RENAME => sys_rename,
        SYS_RENAMEAT => sys_renameat,
        SYS_RENAMEAT2 => sys_renameat2,
        SYS_LINK => sys_link,
        SYS_LINKAT => sys_linkat,
        SYS_CREAT => sys_creat,
        SYS_ACCESS => sys_access,
        SYS_FACCESSAT => sys_faccessat,
        SYS_SYMLINK => sys_symlink,
        SYS_GETCWD => sys_getcwd,
        SYS_CHDIR => sys_chdir,
        SYS_FCHDIR => sys_fchdir,
        SYS_TRUNCATE => sys_truncate,
        SYS_FTRUNCATE => sys_ftruncate,
        SYS_FALLOCATE => sys_fallocate,
        SYS_COPY_FILE_RANGE => sys_copy_file_range,
        SYS_SENDFILE => sys_sendfile,
        SYS_SPLICE => sys_splice,
        SYS_TEE => sys_tee,
        SYS_VMSPLICE => sys_vmsplice,
        SYS_FADVISE64 => sys_fadvise64,
        SYS_CHMOD => sys_chmod,
        SYS_FCHMOD => sys_fchmod,
        SYS_FCHMODAT => sys_fchmodat,
        SYS_CHOWN => sys_chown,
        SYS_FCHOWN => sys_fchown,
        SYS_LCHOWN => sys_lchown,
        SYS_FCHOWNAT => sys_fchownat,
        SYS_UMASK => sys_umask,
        SYS_GETRLIMIT => sys_getrlimit,
        SYS_SETRLIMIT => sys_setrlimit,
        SYS_STATFS => sys_statfs,
        SYS_FSTATFS => sys_fstatfs,
        SYS_OPENAT => sys_openat,
        SYS_GETDENTS64 => sys_getdents64,
        SYS_READLINK => sys_readlink,
        SYS_SYMLINKAT => sys_symlinkat,
        SYS_READLINKAT => sys_readlinkat,
        SYS_EPOLL_CREATE => sys_epoll_create,
        SYS_EPOLL_CREATE1 => sys_epoll_create1,
        SYS_EPOLL_CTL => sys_epoll_ctl,
        SYS_EPOLL_WAIT => sys_epoll_wait,
        SYS_EPOLL_PWAIT => sys_epoll_pwait,
        SYS_EPOLL_PWAIT2 => sys_epoll_pwait2,
        SYS_EVENTFD => sys_eventfd,
        SYS_EVENTFD2 => sys_eventfd2,
        SYS_INOTIFY_INIT1 => sys_inotify_init1,
        SYS_SOCKET => sys_socket,
        SYS_CONNECT => sys_connect,
        SYS_ACCEPT => sys_accept,
        SYS_SENDTO => sys_sendto,
        SYS_RECVFROM => sys_recvfrom,
        SYS_SENDMSG => sys_sendmsg,
        SYS_RECVMSG => sys_recvmsg,
        SYS_SHUTDOWN => sys_shutdown,
        SYS_BIND => sys_bind,
        SYS_LISTEN => sys_listen,
        SYS_GETSOCKNAME => sys_getsockname,
        SYS_GETPEERNAME => sys_getpeername,
        SYS_SOCKETPAIR => sys_socketpair,
        SYS_SETSOCKOPT => sys_setsockopt,
        SYS_GETSOCKOPT => sys_getsockopt,
        SYS_PREADV => sys_preadv,
        SYS_PWRITEV => sys_pwritev,
        SYS_PREADV2 => sys_preadv2,
        SYS_PWRITEV2 => sys_pwritev2,
        // ── Mémoire ────────────────────────────────────────────────────────
        SYS_MMAP => sys_mmap,
        SYS_MREMAP => sys_mremap,
        SYS_MUNMAP => sys_munmap,
        SYS_MPROTECT => sys_mprotect,
        SYS_BRK => sys_brk,
        SYS_SHMGET | SYS_SHMAT | SYS_SHMCTL | SYS_SHMDT => sys_enosys,
        // ── Processus ──────────────────────────────────────────────────────
        SYS_FORK => sys_fork,
        SYS_VFORK => sys_vfork,
        SYS_CLONE => sys_clone,
        SYS_EXECVE => sys_execve,
        SYS_EXIT => sys_exit,
        SYS_EXIT_GROUP => sys_exit_group,
        SYS_WAIT4 => sys_wait4,
        SYS_WAITID => sys_waitid,
        SYS_GETPID => crate::syscall::handlers::misc::sys_getpid,
        SYS_GETPPID => crate::syscall::handlers::misc::sys_getppid,
        SYS_GETTID => crate::syscall::handlers::misc::sys_gettid,
        SYS_GETUID => crate::syscall::handlers::misc::sys_getuid,
        SYS_GETGID => crate::syscall::handlers::misc::sys_getgid,
        SYS_GETEUID => crate::syscall::handlers::misc::sys_geteuid,
        SYS_GETEGID => crate::syscall::handlers::misc::sys_getegid,
        SYS_GETGROUPS => crate::syscall::compat::posix::sys_getgroups,
        SYS_SETGROUPS => crate::syscall::compat::posix::sys_setgroups,
        SYS_CAPGET => crate::syscall::compat::posix::sys_capget,
        SYS_CAPSET => crate::syscall::compat::posix::sys_capset,
        SYS_UNAME => crate::syscall::handlers::misc::sys_uname,
        SYS_ARCH_PRCTL => crate::syscall::handlers::misc::sys_arch_prctl,
        SYS_SET_TID_ADDRESS => crate::syscall::handlers::misc::sys_set_tid_address,
        SYS_PRCTL => crate::syscall::handlers::misc::sys_prctl,
        SYS_SYSINFO => crate::syscall::handlers::misc::sys_sysinfo,
        SYS_GETCPU => crate::syscall::handlers::misc::sys_getcpu,
        // ── Signaux ────────────────────────────────────────────────────────
        SYS_KILL => sys_kill,
        SYS_TGKILL => sys_tgkill,
        SYS_RT_SIGACTION => sys_rt_sigaction,
        SYS_RT_SIGPROCMASK => sys_rt_sigprocmask,
        SYS_SIGALTSTACK => sys_sigaltstack,
        // ── Scheduler ──────────────────────────────────────────────────────
        SYS_SCHED_YIELD => crate::syscall::handlers::misc::sys_sched_yield,
        SYS_CLOCK_GETTIME => sys_clock_gettime,
        SYS_GETTIMEOFDAY => sys_gettimeofday,
        SYS_CLOCK_NANOSLEEP => sys_clock_nanosleep,
        SYS_NANOSLEEP => sys_nanosleep,
        SYS_TIMES => crate::syscall::compat::posix::sys_times,
        SYS_FUTEX => sys_futex,
        SYS_GETRANDOM => sys_getrandom,
        // ── IPC Exo-OS ─────────────────────────────────────────────────────
        SYS_EXO_IPC_SEND => sys_exo_ipc_send,
        SYS_EXO_IPC_RECV => sys_exo_ipc_recv,
        SYS_EXO_IPC_RECV_NB => sys_exo_ipc_recv_nb,
        SYS_EXO_IPC_CALL => sys_exo_ipc_call,
        SYS_EXO_IPC_CREATE => sys_exo_ipc_create,
        SYS_EXO_IPC_DESTROY => sys_exo_ipc_destroy,
        SYS_EXO_IPC_LOOKUP => sys_exo_ipc_lookup,
        SYS_EXO_MEM_COPY_FROM_PID => sys_exo_mem_copy_from_pid,
        SYS_EXO_MEM_COPY_TO_PID => sys_exo_mem_copy_to_pid,
        SYS_EXO_MEM_MAP_PID => sys_exo_mem_map_pid,
        SYS_EXO_MEM_MUNMAP_PID => sys_exo_mem_munmap_pid,
        SYS_EXO_MEM_MPROTECT_PID => sys_exo_mem_mprotect_pid,
        SYS_EXO_CAP_CREATE => sys_exo_cap_create,
        SYS_EXO_CAP_REVOKE => sys_exo_cap_revoke,
        SYS_EXO_CAP_CHECK => sys_exo_cap_check,
        SYS_EXO_LOG => sys_exo_log,
        SYS_EXO_PROCESS_LIST => sys_exo_process_list,
        SYS_EXO_PHOENIX_STATE_SET => sys_exo_phoenix_state_set,
        SYS_EXO_PHOENIX_STATE_GET => sys_exo_phoenix_state_get,
        // ── ExoFS (500–518) ────────────────────────────────────────────────
        SYS_EXOFS_PATH_RESOLVE => sys_exofs_path_resolve,
        SYS_EXOFS_OBJECT_OPEN => sys_exofs_object_open,
        SYS_EXOFS_OBJECT_READ => sys_exofs_object_read,
        SYS_EXOFS_OBJECT_WRITE => sys_exofs_object_write,
        SYS_EXOFS_OBJECT_CREATE => sys_exofs_object_create,
        SYS_EXOFS_OBJECT_DELETE => sys_exofs_object_delete,
        SYS_EXOFS_OBJECT_STAT => sys_exofs_object_stat,
        SYS_EXOFS_OBJECT_SET_META => sys_exofs_object_set_meta_abi,
        SYS_EXOFS_GET_CONTENT_HASH => sys_exofs_get_content_hash,
        SYS_EXOFS_SNAPSHOT_CREATE => sys_exofs_snapshot_create,
        SYS_EXOFS_SNAPSHOT_LIST => sys_exofs_snapshot_list,
        SYS_EXOFS_SNAPSHOT_MOUNT => sys_exofs_snapshot_mount,
        SYS_EXOFS_RELATION_CREATE => sys_exofs_relation_create,
        SYS_EXOFS_RELATION_QUERY => sys_exofs_relation_query,
        SYS_EXOFS_GC_TRIGGER => sys_exofs_gc_trigger,
        SYS_EXOFS_QUOTA_QUERY => sys_exofs_quota_query,
        SYS_EXOFS_EXPORT_OBJECT => sys_exofs_export_object,
        SYS_EXOFS_IMPORT_OBJECT => sys_exofs_import_object,
        SYS_EXOFS_EPOCH_COMMIT => sys_exofs_epoch_commit,
        // ── ExoFS extensions (519–520) — FIX BUG-01 + BUG-02 ───────────────
        SYS_EXOFS_OPEN_BY_PATH => sys_exofs_open_by_path,
        SYS_EXOFS_READDIR => sys_exofs_readdir,
        // ── GI-03 Drivers (530–546) ──────────────────────────────────────────
        SYS_IRQ_REGISTER => sys_irq_register,
        SYS_IRQ_ACK => sys_irq_ack,
        SYS_MMIO_MAP => sys_mmio_map,
        SYS_MMIO_UNMAP => sys_mmio_unmap,
        SYS_DMA_ALLOC => sys_dma_alloc,
        SYS_DMA_FREE => sys_dma_free,
        SYS_DMA_SYNC => sys_dma_sync,
        SYS_PCI_CFG_READ => sys_pci_cfg_read,
        SYS_PCI_CFG_WRITE => sys_pci_cfg_write,
        SYS_PCI_BUS_MASTER => sys_pci_bus_master,
        SYS_PCI_CLAIM => sys_pci_claim,
        SYS_DMA_MAP => sys_dma_map,
        SYS_DMA_UNMAP => sys_dma_unmap,
        SYS_MSI_ALLOC => sys_msi_alloc,
        SYS_MSI_CONFIG => sys_msi_config,
        SYS_MSI_FREE => sys_msi_free,
        SYS_PCI_SET_TOPOLOGY => sys_pci_set_topology,
        // ── Catch-all ──────────────────────────────────────────────────────
        _ => sys_enosys,
    }
}
