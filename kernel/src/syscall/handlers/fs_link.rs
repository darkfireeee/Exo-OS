//! File Link and Rename Syscalls
//!
//! Implements:
//! - link, symlink, readlink
//! - unlink, unlinkat
//! - rename, renameat

use crate::fs::vfs::inode::InodeType;
use crate::fs::{FsError, FsResult};
use crate::posix_x::vfs_posix::{file_ops, path_resolver};
use crate::syscall::utils::{copy_to_user, read_user_string};
use alloc::string::ToString;

/// sys_link - Create a new name for a file (hard link)
///
/// Note: Current VFS Inode trait does not support hard links (aliasing inodes).
/// This is a stub that returns ENOSYS.
pub unsafe fn sys_link(_oldpath: *const i8, _newpath: *const i8) -> i64 {
    // TODO: Extend Inode trait to support hard links (link/linkat)
    -38 // ENOSYS
}

/// sys_symlink - Create a symbolic link
pub unsafe fn sys_symlink(target_ptr: *const i8, linkpath_ptr: *const i8) -> i64 {
    let target = match read_user_string(target_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    let linkpath = match read_user_string(linkpath_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    // 1. Resolve parent directory of linkpath
    let (parent_inode, filename) = match path_resolver::resolve_parent(&linkpath) {
        Ok(res) => res,
        Err(e) => return -(e.to_errno() as i64),
    };

    // 2. Create symlink inode
    let new_ino = {
        let mut parent = parent_inode.write();
        match parent.create(&filename, InodeType::Symlink) {
            Ok(ino) => ino,
            Err(e) => return -(e.to_errno() as i64),
        }
    };

    // 3. Get the new inode
    let inode = match crate::posix_x::vfs_posix::inode_cache::get_inode(new_ino) {
        Ok(i) => i,
        Err(e) => return -(e.to_errno() as i64),
    };

    // 4. Write target path into the inode
    // Symlinks store the target path as file content
    let mut inode_guard = inode.write();
    match inode_guard.write_at(0, target.as_bytes()) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_readlink - Read value of a symbolic link
pub unsafe fn sys_readlink(path_ptr: *const i8, buf: *mut u8, bufsiz: usize) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    // Resolve path, but DO NOT follow the final symlink (we want to read it)
    let inode = match path_resolver::resolve_path(&path, None, false) {
        Ok(i) => i,
        Err(e) => return -(e.to_errno() as i64),
    };

    let inode_guard = inode.read();

    // Verify it is a symlink
    if inode_guard.inode_type() != InodeType::Symlink {
        return -22; // EINVAL
    }

    // Read content
    // We need a temporary buffer because read_at takes a slice
    // and we want to avoid holding the lock while copying to user if possible,
    // but read_at copies directly.
    // However, we are copying to user memory `buf`.
    // We can't pass user pointer directly to read_at if it expects a kernel slice.
    // read_at takes `&mut [u8]`. We can create a slice from raw parts if we trust the pointer?
    // No, that's unsafe and bypasses SMAP/SMEP if enabled (though we are in kernel).
    // Better to read to kernel buffer then copy to user.

    let size = inode_guard.size() as usize;
    let read_len = core::cmp::min(size, bufsiz);
    let mut kbuf = alloc::vec![0u8; read_len];

    match inode_guard.read_at(0, &mut kbuf) {
        Ok(n) => {
            if copy_to_user(buf, &kbuf[..n]).is_err() {
                return -14; // EFAULT
            }
            n as i64
        }
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_unlink - Remove a file
pub unsafe fn sys_unlink(path_ptr: *const i8) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    match file_ops::unlink(&path) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_unlinkat - Remove a directory entry relative to a directory file descriptor
pub unsafe fn sys_unlinkat(dirfd: i32, path_ptr: *const i8, flags: i32) -> i64 {
    let path = match read_user_string(path_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    // AT_REMOVEDIR (0x200) -> rmdir
    if (flags & 0x200) != 0 {
        return crate::syscall::handlers::fs_dir::sys_rmdir(path_ptr);
    }

    // If path is absolute, ignore dirfd
    if path.starts_with('/') {
        return sys_unlink(path_ptr);
    }

    // Handle AT_FDCWD
    if dirfd == -100 {
        return sys_unlink(path_ptr);
    }

    // Resolve dirfd
    // TODO: Implement full *at support in file_ops or path_resolver
    // For now, we construct the full path manually if possible, or fail.
    // This is a limitation of current file_ops which take &str path.

    // Get dirfd path
    let process = match crate::posix_x::core::process_state::current_process_state() {
        Some(p) => p,
        None => return -3, // ESRCH
    };

    let fd_path = {
        let state = process.read();
        match state.fd_table.get(dirfd) {
            Some(handle) => handle.read().path().to_string(),
            None => return -9, // EBADF
        }
    };

    let full_path = if fd_path.ends_with('/') {
        alloc::format!("{}{}", fd_path, path)
    } else {
        alloc::format!("{}/{}", fd_path, path)
    };

    match file_ops::unlink(&full_path) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_rename - Rename a file or directory
pub unsafe fn sys_rename(oldpath_ptr: *const i8, newpath_ptr: *const i8) -> i64 {
    let oldpath = match read_user_string(oldpath_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    let newpath = match read_user_string(newpath_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    match file_ops::rename(&oldpath, &newpath) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}

/// sys_renameat - Rename relative to directory file descriptors
pub unsafe fn sys_renameat(
    olddirfd: i32,
    oldpath_ptr: *const i8,
    newdirfd: i32,
    newpath_ptr: *const i8,
) -> i64 {
    // Similar logic to unlinkat, construct full paths if needed
    let oldpath = match read_user_string(oldpath_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    let newpath = match read_user_string(newpath_ptr) {
        Ok(s) => s,
        Err(_) => return -14, // EFAULT
    };

    let resolve_at = |dirfd: i32, path: &str| -> Result<alloc::string::String, i64> {
        if path.starts_with('/') || dirfd == -100 {
            Ok(path.to_string())
        } else {
            let process = match crate::posix_x::core::process_state::current_process_state() {
                Some(p) => p,
                None => return Err(-3), // ESRCH
            };
            let state = process.read();
            match state.fd_table.get(dirfd) {
                Some(handle) => {
                    let fd_path = handle.read().path().to_string();
                    if fd_path.ends_with('/') {
                        Ok(alloc::format!("{}{}", fd_path, path))
                    } else {
                        Ok(alloc::format!("{}/{}", fd_path, path))
                    }
                }
                None => Err(-9), // EBADF
            }
        }
    };

    let full_old = match resolve_at(olddirfd, &oldpath) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let full_new = match resolve_at(newdirfd, &newpath) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match file_ops::rename(&full_old, &full_new) {
        Ok(_) => 0,
        Err(e) => -(e.to_errno() as i64),
    }
}
