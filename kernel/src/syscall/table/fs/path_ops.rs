//! # syscall/table/fs/path_ops.rs — Handlers d'opérations sur les chemins VFS et métadonnées

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::{read_user_path, validate_fd};

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
