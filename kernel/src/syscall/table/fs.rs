//! # syscall/table/fs.rs — Handlers pour I/O, descripteurs de fichiers et ExoFS

use super::common::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    copy_from_user, copy_to_user, read_user_path, validate_fd, validate_flags, UserBuf, IO_BUF_MAX,
};
pub use crate::fs::exofs::syscall::{
    sys_exofs_epoch_commit, sys_exofs_export_object, sys_exofs_gc_trigger,
    sys_exofs_get_content_hash, sys_exofs_import_object, sys_exofs_object_create,
    sys_exofs_object_delete, sys_exofs_object_open, sys_exofs_object_read,
    sys_exofs_object_set_meta, sys_exofs_object_stat, sys_exofs_object_write,
    sys_exofs_open_by_path, sys_exofs_path_resolve, sys_exofs_quota_query, sys_exofs_readdir,
    sys_exofs_relation_create, sys_exofs_relation_query, sys_exofs_snapshot_create,
    sys_exofs_snapshot_list, sys_exofs_snapshot_mount,
};

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
    use crate::syscall::net_bridge;
    if let Some(socket_fd) = net_bridge::socket_handle_from_raw(fd) {
        return net_bridge::bridge_result(net_bridge::net_close(socket_fd));
    }
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

const LINUX_REBOOT_MAGIC1: u64 = 0xfee1_dead;
const LINUX_REBOOT_MAGIC2: u64 = 672_274_793;
const LINUX_REBOOT_CMD_RESTART: u64 = 0x0123_4567;
const LINUX_REBOOT_CMD_POWER_OFF: u64 = 0x4321_fedc;
const QEMU_DEBUG_EXIT_PORT: u16 = 0x00f4;
const QEMU_DEBUG_EXIT_SUCCESS: u32 = 0x10;
const PS2_CONTROLLER_PORT: u16 = 0x0064;
const PS2_RESET_COMMAND: u8 = 0xfe;

#[inline(never)]
fn halt_after_reboot_request() -> ! {
    crate::arch::halt_cpu()
}

#[inline(never)]
#[cfg(target_arch = "x86_64")]
fn platform_poweroff() -> ! {
    // SAFETY: SYS_REBOOT runs in ring 0. Port 0xf4 is the configured
    // QEMU isa-debug-exit device in the project boot commands.
    unsafe {
        crate::arch::x86_64::outl(QEMU_DEBUG_EXIT_PORT, QEMU_DEBUG_EXIT_SUCCESS);
    }
    halt_after_reboot_request()
}

#[inline(never)]
#[cfg(not(target_arch = "x86_64"))]
fn platform_poweroff() -> ! {
    halt_after_reboot_request()
}

#[inline(never)]
#[cfg(target_arch = "x86_64")]
fn platform_restart() -> ! {
    // SAFETY: SYS_REBOOT runs in ring 0. 0x64/0xfe is the legacy PS/2
    // controller CPU reset command supported by QEMU and many PCs.
    unsafe {
        crate::arch::x86_64::outb(PS2_CONTROLLER_PORT, PS2_RESET_COMMAND);
    }
    halt_after_reboot_request()
}

#[inline(never)]
#[cfg(not(target_arch = "x86_64"))]
fn platform_restart() -> ! {
    halt_after_reboot_request()
}

/// `reboot(magic1, magic2, cmd, arg)` compatible Linux/musl.
pub fn sys_reboot(magic1: u64, magic2: u64, cmd: u64, _arg: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_REBOOT);
    if magic1 != LINUX_REBOOT_MAGIC1 || magic2 != LINUX_REBOOT_MAGIC2 {
        return EINVAL;
    }

    // Only PID 0 (kernel boot context) or PID 1 (init) may reboot the system.
    // Any other process knowing the magic constants could otherwise DoS the system.
    let caller = current_pid_u32();
    if caller > 1 {
        crate::security::shield_feed::push_event(
            caller,
            crate::security::shield_feed::event_type::SYSCALL,
            crate::security::shield_feed::severity::CRITICAL,
            crate::syscall::numbers::SYS_REBOOT as u32,
            magic1,
            cmd,
        );
        return EPERM;
    }

    use crate::syscall::fs_bridge;
    if let Err(err) = fs_bridge::fs_sync(current_pid_u32()) {
        return err.to_errno();
    }

    match cmd {
        LINUX_REBOOT_CMD_POWER_OFF => platform_poweroff(),
        LINUX_REBOOT_CMD_RESTART => platform_restart(),
        _ => EINVAL,
    }
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

