//! # syscall/table/fs/io.rs — Handlers I/O de lecture/écriture, vecteurs et flux

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::{validate_fd, UserBuf, IO_BUF_MAX};

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
        return EINVAL;
    }
    sys_preadv(fd, iov_ptr, iovcnt, offset, 0, 0)
}

/// `pwritev2(fd, iov, iovcnt, offset, flags)`.
pub fn sys_pwritev2(fd: u64, iov_ptr: u64, iovcnt: u64, offset: u64, flags: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PWRITEV2);
    if flags != 0 {
        return EINVAL;
    }
    sys_pwritev(fd, iov_ptr, iovcnt, offset, 0, 0)
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
