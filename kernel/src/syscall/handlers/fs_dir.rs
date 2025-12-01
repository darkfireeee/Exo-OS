//! Directory Operations Syscalls
//!
//! Implements:
//! - mkdir, rmdir
//! - getcwd, chdir, fchdir
//! - getdents64

use crate::fs::vfs::inode::InodeType;
use crate::fs::{FsError, FsResult};
use crate::posix_x::core::process_state::current_process_state;
use crate::posix_x::vfs_posix::{file_ops, path_resolver, VfsHandle};
use crate::syscall::utils::{copy_to_user, read_user_string, write_user_type};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem::size_of;

/// linux_dirent64 structure
#[repr(C, packed)]
struct LinuxDirent64 {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
    // d_name follows
}

/// sys_mkdir - Create a directory
pub unsafe fn sys_mkdir(path_ptr: *const i8, mode: u32) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    match file_ops::mkdir(&path, mode) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_rmdir - Remove a directory
pub unsafe fn sys_rmdir(path_ptr: *const i8) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    match file_ops::rmdir(&path) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_getcwd - Get current working directory
pub unsafe fn sys_getcwd(buf: *mut u8, size: usize) -> i64 {
    let process = match current_process_state() {
        Some(p) => p,
        None => return -3, // ESRCH
    };

    let cwd = process.read().cwd.clone();
    let len = cwd.len() + 1; // +1 for null terminator

    if size < len {
        return -34; // ERANGE
    }

    // Copy to user buffer
    if copy_to_user(buf, cwd.as_bytes()).is_err() {
        return -14; // EFAULT
    }

    // Null terminate
    if copy_to_user(buf.add(cwd.len()), &[0]).is_err() {
        return -14; // EFAULT
    }

    len as i64
}

/// sys_chdir - Change working directory
pub unsafe fn sys_chdir(path_ptr: *const i8) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    // Verify path exists and is a directory
    // We need to resolve it relative to current CWD if it's relative
    let process = match current_process_state() {
        Some(p) => p,
        None => return -3, // ESRCH
    };

    let cwd_path = process.read().cwd.clone();
    let cwd_inode = match path_resolver::resolve_path(&cwd_path, None, true) {
        Ok(inode) => Some(inode),
        Err(_) => None, // Should not happen if CWD is valid
    };

    match path_resolver::resolve_path(&path, cwd_inode, true) {
        Ok(inode) => {
            if inode.read().inode_type() != InodeType::Directory {
                return -20; // ENOTDIR
            }

            // Update CWD
            let new_cwd = if path.starts_with('/') {
                path
            } else {
                let mut p = cwd_path;
                if !p.ends_with('/') {
                    p.push('/');
                }
                p.push_str(&path);
                // TODO: Normalize (remove . and ..)
                p
            };

            process.write().chdir(new_cwd);
            0
        }
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_fchdir - Change working directory by FD
pub unsafe fn sys_fchdir(fd: i32) -> i64 {
    let process = match current_process_state() {
        Some(p) => p,
        None => return -3, // ESRCH
    };

    let mut state = process.write();
    let handle = match state.fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };

    let handle_guard = handle.read();
    let inode = handle_guard.inode();

    if inode.read().inode_type() != InodeType::Directory {
        return -20; // ENOTDIR
    }

    // Update CWD
    // VfsHandle stores the path!
    state.chdir(handle_guard.path().to_string());

    0
}

/// sys_getdents64 - Get directory entries
pub unsafe fn sys_getdents64(fd: i32, dirp: *mut u8, count: usize) -> i64 {
    let process = match current_process_state() {
        Some(p) => p,
        None => return -3, // ESRCH
    };

    let handle = match process.read().fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };

    // Use the handle's path to list directory
    let handle_guard = handle.read();
    let path = handle_guard.path();

    let entries = match file_ops::readdir(path) {
        Ok(e) => e,
        Err(e) => return -(e.to_errno() as i64),
    };

    // Serialize to linux_dirent64
    let mut offset = 0;
    let mut bytes_written = 0;

    for (_i, name) in entries.iter().enumerate() {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len();
        let reclen = (size_of::<LinuxDirent64>() + name_len + 1 + 7) & !7; // Align to 8 bytes

        if bytes_written + reclen > count {
            break;
        }

        let dirent = LinuxDirent64 {
            d_ino: 1, // TODO: Get real inode number
            d_off: (offset + 1) as i64,
            d_reclen: reclen as u16,
            d_type: 0, // DT_UNKNOWN - let userspace stat if needed
        };

        // Write header
        let ptr = dirp.add(bytes_written);
        // We can't just write the struct because of the flexible array member
        // Manually write fields
        *(ptr as *mut u64) = dirent.d_ino;
        *(ptr.add(8) as *mut i64) = dirent.d_off;
        *(ptr.add(16) as *mut u16) = dirent.d_reclen;
        *(ptr.add(18) as *mut u8) = dirent.d_type;

        // Write name
        if copy_to_user(ptr.add(19), name_bytes).is_err() {
            return -14; // EFAULT
        }
        *ptr.add(19 + name_len) = 0; // Null terminator

        bytes_written += reclen;
        offset += 1;
    }

    bytes_written as i64
}
