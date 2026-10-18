//! # syscall/table/fs/dirs.rs — Handlers pour la navigation dans les répertoires et état FS processus

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::{read_user_path, validate_fd};

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

/// `sync()`.
pub fn sys_sync(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SYNC);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_sync(pid))
}
