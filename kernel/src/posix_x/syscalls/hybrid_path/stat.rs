//! Metadata Syscalls - Phase 8 Implementation
//!
//! POSIX stat syscalls integrated with VFS adapter.

use crate::fs::FsError;
use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
use crate::posix_x::vfs_posix::{file_ops, FileStat};
use core::ffi::CStr;

/// POSIX stat structure (subset of full stat)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PosixStat {
    pub st_dev: u64,     // Device ID
    pub st_ino: u64,     // Inode number
    pub st_mode: u32,    // File type and mode
    pub st_nlink: u64,   // Number of hard links
    pub st_uid: u32,     // User ID
    pub st_gid: u32,     // Group ID
    pub st_rdev: u64,    // Device ID (if special file)
    pub st_size: i64,    // Total size in bytes
    pub st_blksize: i64, // Block size for filesystem I/O
    pub st_blocks: i64,  // Number of 512B blocks allocated
    pub st_atime: i64,   // Time of last access
    pub st_mtime: i64,   // Time of last modification
    pub st_ctime: i64,   // Time of last status change
}

/// Convert VFS FileStat to POSIX stat
impl From<FileStat> for PosixStat {
    fn from(fs: FileStat) -> Self {
        Self {
            st_dev: 0,         // TODO: Real device ID
            st_ino: 0,         // fs.inode_id - not available yet
            st_mode: 0o100644, // fs.mode - default regular file, rw-r--r--
            st_nlink: 1,       // fs.nlinks
            st_uid: 1000,      // fs.uid - default user
            st_gid: 1000,      // fs.gid - default group
            st_rdev: 0,
            st_size: fs.size as i64,
            st_blksize: 4096, // Standard block size
            st_blocks: ((fs.size + 511) / 512) as i64,
            st_atime: 0, // fs.atime - not available yet
            st_mtime: 0, // fs.mtime
            st_ctime: 0, // fs.ctime
        }
    }
}

/// Convert FsError to errno
fn fs_error_to_errno(e: FsError) -> i32 {
    match e {
        FsError::NotFound => 2,          // ENOENT
        FsError::PermissionDenied => 13, // EACCES
        FsError::AlreadyExists => 17,    // EEXIST
        FsError::InvalidFd => 9,         // EBADF
        FsError::TooManyFiles => 24,     // EMFILE
        _ => 5,                          // EIO
    }
}

/// stat(pathname, statbuf) - Get file status
#[no_mangle]
pub unsafe extern "C" fn sys_stat(pathname: *const i8, statbuf: *mut PosixStat) -> i64 {
    if pathname.is_null() || statbuf.is_null() {
        return -14; // -EFAULT
    }

    // Convert C string to Rust
    let path = match CStr::from_ptr(pathname).to_str() {
        Ok(s) => s,
        Err(_) => return -22, // -EINVAL
    };

    // Get metadata via VFS (follow symlinks)
    let vfs_stat = match file_ops::stat(path, true) {
        Ok(s) => s,
        Err(e) => return -(fs_error_to_errno(e) as i64),
    };

    // Convert and write to userspace
    *statbuf = PosixStat::from(vfs_stat);
    0
}

/// fstat(fd, statbuf) - Get file status by FD
#[no_mangle]
pub unsafe extern "C" fn sys_fstat(fd: i32, statbuf: *mut PosixStat) -> i64 {
    if statbuf.is_null() {
        return -14; // -EFAULT
    }

    // Get VFS handle
    let table = GLOBAL_FD_TABLE.read();
    let handle_arc = match table.get(fd) {
        Some(h) => h,
        None => return -9, // -EBADF
    };
    drop(table);

    // Get metadata from handle
    let handle = handle_arc.read();
    let vfs_stat = match handle.stat() {
        Ok(s) => s,
        Err(e) => return -(fs_error_to_errno(e) as i64),
    };

    // Convert and write
    *statbuf = PosixStat::from(vfs_stat);
    0
}

/// lstat(pathname, statbuf) - Like stat but don't follow symlinks
#[no_mangle]
pub unsafe extern "C" fn sys_lstat(pathname: *const i8, statbuf: *mut PosixStat) -> i64 {
    if pathname.is_null() || statbuf.is_null() {
        return -14; // -EFAULT
    }

    // Convert C string
    let path = match CStr::from_ptr(pathname).to_str() {
        Ok(s) => s,
        Err(_) => return -22, // -EINVAL
    };

    // Get metadata (DON'T follow symlinks)
    let vfs_stat = match file_ops::stat(path, false) {
        Ok(s) => s,
        Err(e) => return -(fs_error_to_errno(e) as i64),
    };

    // Convert and write
    *statbuf = PosixStat::from(vfs_stat);
    0
}
