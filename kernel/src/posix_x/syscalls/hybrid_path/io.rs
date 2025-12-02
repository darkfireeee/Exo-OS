//! I/O Syscalls - read/write/open/close with real VFS integration

use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
use crate::posix_x::translation::errno::Errno;
use crate::posix_x::vfs_posix::file_ops;
use alloc::sync::Arc;
use core::ffi::CStr;
use core::slice;
use spin::RwLock;

/// Read from file descriptor
pub fn sys_read(fd: i32, buf: usize, count: usize) -> i64 {
    // Validate pointers
    if buf == 0 || count == 0 {
        return -(Errno::EFAULT as i64);
    }

    // Get FD handle
    let table = GLOBAL_FD_TABLE.read();
    let handle_arc = match table.get(fd) {
        Some(h) => h.clone(),
        None => return -(Errno::EBADF as i64),
    };
    drop(table);

    // Read from handle
    let mut handle = handle_arc.write();
    let buffer = unsafe { slice::from_raw_parts_mut(buf as *mut u8, count) };

    match handle.read(buffer) {
        Ok(bytes_read) => bytes_read as i64,
        Err(_e) => -(Errno::EIO as i64),
    }
}

/// Write to file descriptor
pub fn sys_write(fd: i32, buf: usize, count: usize) -> i64 {
    // Validate pointers
    if buf == 0 || count == 0 {
        return 0; // Writing 0 bytes is success
    }

    // Get FD handle
    let table = GLOBAL_FD_TABLE.read();
    let handle_arc = match table.get(fd) {
        Some(h) => h.clone(),
        None => return -(Errno::EBADF as i64),
    };
    drop(table);

    // Write to handle
    let mut handle = handle_arc.write();
    let buffer = unsafe { slice::from_raw_parts(buf as *const u8, count) };

    match handle.write(buffer) {
        Ok(bytes_written) => bytes_written as i64,
        Err(_e) => -(Errno::EIO as i64),
    }
}

/// Open file
pub fn sys_open(pathname: usize, flags: i32, mode: u32) -> i64 {
    if pathname == 0 {
        return -(Errno::EFAULT as i64);
    }

    // Convert C string to Rust
    let path = unsafe {
        match CStr::from_ptr(pathname as *const i8).to_str() {
            Ok(s) => s,
            Err(_) => return -(Errno::EINVAL as i64),
        }
    };

    // Convert POSIX flags to OpenFlags
    use crate::posix_x::vfs_posix::OpenFlags;
    let open_flags = OpenFlags::from_posix(flags);

    // Open via VFS (no cwd_inode for now)
    match file_ops::open(path, open_flags, mode, None) {
        Ok(handle) => {
            // Allocate FD
            let mut table = GLOBAL_FD_TABLE.write();
            match table.allocate(handle) {
                Ok(fd) => fd as i64,
                Err(_) => -(Errno::EMFILE as i64),
            }
        }
        Err(fs_error) => {
            // Convert FsError to errno
            use crate::fs::FsError;
            let errno = match fs_error {
                FsError::NotFound => Errno::ENOENT,
                FsError::PermissionDenied => Errno::EACCES,
                FsError::AlreadyExists => Errno::EEXIST,
                FsError::InvalidFd => Errno::EBADF,
                _ => Errno::EIO,
            };
            -(errno as i64)
        }
    }
}

/// Close file descriptor
pub fn sys_close(fd: i32) -> i64 {
    let mut table = GLOBAL_FD_TABLE.write();
    match table.close(fd) {
        Ok(()) => 0,
        Err(_) => -(Errno::EBADF as i64),
    }
}

/// Seek in file
pub fn sys_lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    // Get FD handle
    let table = GLOBAL_FD_TABLE.read();
    let handle_arc = match table.get(fd) {
        Some(h) => h.clone(),
        None => return -(Errno::EBADF as i64),
    };
    drop(table);

    // Seek
    use crate::posix_x::vfs_posix::SeekWhence;
    let seek_whence = match whence {
        0 => SeekWhence::Set, // SEEK_SET
        1 => SeekWhence::Cur, // SEEK_CUR
        2 => SeekWhence::End, // SEEK_END
        _ => return -(Errno::EINVAL as i64),
    };

    let mut handle = handle_arc.write();
    match handle.seek(seek_whence, offset) {
        Ok(new_offset) => new_offset as i64,
        Err(_) => -(Errno::EINVAL as i64),
    }
}

/// ioctl - Device control
pub fn sys_ioctl(_fd: i32, _request: u64, _argp: usize) -> i64 {
    // Basic ioctl support
    -(Errno::ENOTTY as i64)
}

/// fsync - Sync file to disk
pub fn sys_fsync(fd: i32) -> i64 {
    // Get FD handle
    let table = GLOBAL_FD_TABLE.read();
    let handle_arc = match table.get(fd) {
        Some(h) => h.clone(),
        None => return -(Errno::EBADF as i64),
    };
    drop(table);

    // Flush (VFS handle automatically flushes on write)
    // For now, just return success
    0
}

/// fdatasync - Sync file data (not metadata)
pub fn sys_fdatasync(fd: i32) -> i64 {
    // For now, same as fsync
    sys_fsync(fd)
}
