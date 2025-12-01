//! I/O Syscalls - Phase 8 Implementation
//!
//! POSIX I/O syscalls integrated with VFS adapter.

use crate::fs::FsError;
use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
use crate::posix_x::vfs_posix::{file_ops, OpenFlags, SeekWhence};
use core::ffi::CStr;

/// Convert FsError to POSIX errno
fn fs_error_to_errno(e: FsError) -> i32 {
    match e {
        FsError::NotFound => 2,          // ENOENT
        FsError::PermissionDenied => 13, // EACCES
        FsError::FileExists => 17,       // EEXIST
        FsError::NotDirectory => 20,     // ENOTDIR
        FsError::IsDirectory => 21,      // EISDIR
        FsError::InvalidArgument => 22,  // EINVAL
        FsError::TooManyFiles => 24,     // EMFILE
        _ => 5,                          // EIO
    }
}

/// open(pathname, flags, mode)
#[no_mangle]
pub unsafe extern "C" fn sys_open(pathname: *const i8, flags: i32, mode: u32) -> i64 {
    // Validate pointer
    if pathname.is_null() {
        return -14; // -EFAULT
    }

    // Convert C string to Rust
    let path = match CStr::from_ptr(pathname).to_str() {
        Ok(s) => s,
        Err(_) => return -22, // -EINVAL
    };

    // Convert POSIX flags to internal format
    let open_flags = OpenFlags::from_posix(flags);

    // Open file via VFS
    let handle = match file_ops::open(path, open_flags, mode, None) {
        Ok(h) => h,
        Err(e) => {
            let errno = fs_error_to_errno(e);
            return -(errno as i64);
        }
    };

    // Allocate FD
    let mut table = GLOBAL_FD_TABLE.lock();
    match table.allocate(handle) {
        Ok(fd) => fd as i64,
        Err(_) => -24, // -EMFILE
    }
}

/// read(fd, buf, count)
#[no_mangle]
pub unsafe extern "C" fn sys_read(fd: i32, buf: *mut u8, count: usize) -> i64 {
    // Validate buffer
    if buf.is_null() {
        return -14; // -EFAULT
    }

    // Get VFS handle
    let table = GLOBAL_FD_TABLE.lock();
    let handle_arc = match table.get(fd) {
        Some(h) => h,
        None => return -9, // -EBADF
    };
    drop(table);

    // Read from file
    let buffer = core::slice::from_raw_parts_mut(buf, count);
    let mut handle = handle_arc.write();

    match handle.read(buffer) {
        Ok(n) => n as i64,
        Err(e) => -(fs_error_to_errno(e) as i64),
    }
}

/// write(fd, buf, count)
#[no_mangle]
pub unsafe extern "C" fn sys_write(fd: i32, buf: *const u8, count: usize) -> i64 {
    // Validate buffer
    if buf.is_null() {
        return -14; // -EFAULT
    }

    // Special case: stdout/stderr via serial (keep existing behavior)
    if fd == 1 || fd == 2 {
        let slice = core::slice::from_raw_parts(buf, count);
        for &byte in slice {
            crate::logger::early_putc(byte as char);
        }
        return count as i64;
    }

    // Get VFS handle
    let table = GLOBAL_FD_TABLE.lock();
    let handle_arc = match table.get(fd) {
        Some(h) => h,
        None => return -9, // -EBADF
    };
    drop(table);

    // Write to file
    let buffer = core::slice::from_raw_parts(buf, count);
    let mut handle = handle_arc.write();

    match handle.write(buffer) {
        Ok(n) => n as i64,
        Err(e) => -(fs_error_to_errno(e) as i64),
    }
}

/// close(fd)
#[no_mangle]
pub unsafe extern "C" fn sys_close(fd: i32) -> i64 {
    let mut table = GLOBAL_FD_TABLE.lock();
    match table.close(fd) {
        Ok(()) => 0,
        Err(_) => -9, // -EBADF
    }
}

/// lseek(fd, offset, whence)
#[no_mangle]
pub unsafe extern "C" fn sys_lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    // Get handle
    let table = GLOBAL_FD_TABLE.lock();
    let handle_arc = match table.get(fd) {
        Some(h) => h,
        None => return -9, // -EBADF
    };
    drop(table);

    // Convert whence
    let seek_whence = match SeekWhence::from_i32(whence) {
        Some(w) => w,
        None => return -22, // -EINVAL
    };

    // Perform seek
    let mut handle = handle_arc.write();
    match handle.seek(seek_whence, offset) {
        Ok(new_offset) => new_offset as i64,
        Err(e) => -(fs_error_to_errno(e) as i64),
    }
}
