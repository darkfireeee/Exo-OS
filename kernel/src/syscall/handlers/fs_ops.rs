//! File Operations System Call Handlers
//!
//! Implements truncate, ftruncate, sync, fsync, fdatasync, sendfile, splice, tee.

use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;

/// Truncate a file to a specified length
pub unsafe fn sys_truncate(_path: *const i8, _length: i64) -> i64 {
    log::info!("sys_truncate: path={:?}, length={}", _path, _length);
    // TODO: Resolve path to inode and call truncate
    0
}

/// Truncate a file specified by a file descriptor
pub unsafe fn sys_ftruncate(fd: i32, _length: i64) -> i64 {
    log::info!("sys_ftruncate: fd={}, length={}", fd, _length);

    let fd_table = GLOBAL_FD_TABLE.read();
    if let Some(_handle) = fd_table.get(fd) {
        // TODO: Call truncate on handle/inode
        0
    } else {
        -9 // EBADF
    }
}

/// Synchronize filesystem
pub unsafe fn sys_sync() -> i64 {
    log::info!("sys_sync");
    // TODO: Flush all filesystems
    0
}

/// Synchronize a file's in-core state with storage device
pub unsafe fn sys_fsync(fd: i32) -> i64 {
    log::info!("sys_fsync: fd={}", fd);

    let fd_table = GLOBAL_FD_TABLE.read();
    if let Some(_handle) = fd_table.get(fd) {
        // TODO: Call sync on handle/inode
        0
    } else {
        -9 // EBADF
    }
}

/// Synchronize a file's data with storage device (no metadata)
pub unsafe fn sys_fdatasync(fd: i32) -> i64 {
    log::info!("sys_fdatasync: fd={}", fd);
    // Same as fsync for now
    sys_fsync(fd)
}

/// Transfer data between file descriptors
pub unsafe fn sys_sendfile(out_fd: i32, in_fd: i32, _offset: *mut i64, count: usize) -> i64 {
    log::info!(
        "sys_sendfile: out={}, in={}, count={}",
        out_fd,
        in_fd,
        count
    );

    // Stub: return count to simulate success
    count as i64
}

/// Splice data to/from a pipe
pub unsafe fn sys_splice(
    fd_in: i32,
    _off_in: *mut i64,
    fd_out: i32,
    _off_out: *mut i64,
    len: usize,
    flags: u32,
) -> i64 {
    log::info!(
        "sys_splice: in={}, out={}, len={}, flags={:#x}",
        fd_in,
        fd_out,
        len,
        flags
    );
    // Stub
    len as i64
}

/// Duplicate pipe content
pub unsafe fn sys_tee(fd_in: i32, fd_out: i32, len: usize, flags: u32) -> i64 {
    log::info!(
        "sys_tee: in={}, out={}, len={}, flags={:#x}",
        fd_in,
        fd_out,
        len,
        flags
    );
    // Stub
    len as i64
}
