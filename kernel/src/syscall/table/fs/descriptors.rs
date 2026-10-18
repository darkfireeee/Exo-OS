//! # syscall/table/fs/descriptors.rs — Handlers de gestion des descripteurs de fichiers

use crate::fs::exofs::syscall::object_fd::open_flags;
use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::{read_user_path, validate_fd, validate_flags};

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
