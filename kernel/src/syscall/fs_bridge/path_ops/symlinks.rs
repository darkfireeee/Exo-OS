//! # syscall/fs_bridge/path_ops/symlinks.rs — Opérations sur les liens symboliques POSIX

use super::super::*;
use super::resolution::*;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::path::symlink::{is_valid_symlink_target, register_symlink};
use crate::syscall::validation::copy_to_user;

/// `symlink(target, linkpath)`.
#[inline]
pub fn fs_symlink(target: &[u8], linkpath: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if !is_valid_symlink_target(target) {
        return Err(FsBridgeError::Invalid);
    }

    let normalized_link = resolve_path_with_symlinks(linkpath, false, true)?;
    if normalized_link == b"/" {
        return Err(FsBridgeError::BadPath);
    }
    if path_entry(&normalized_link).is_ok() {
        return Err(FsBridgeError::Exists);
    }

    let (parent_path, leaf) = split_parent_and_leaf(&normalized_link)?;
    ensure_directory_exists(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_link)?;
    BLOB_CACHE
        .insert(blob_id, target.to_vec())
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    register_symlink(&blob_id_to_object_id(blob_id), target).map_err(exofs_to_bridge_error)?;
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_SYMLINK)?;
    Ok(0)
}

/// `symlinkat(target, dirfd, linkpath)`.
#[inline]
pub fn fs_symlinkat(
    target: &[u8],
    dirfd: i32,
    linkpath: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || linkpath.starts_with(b"/") {
        return fs_symlink(target, linkpath, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `readlink(path, buf, bufsize)`.
#[inline]
pub fn fs_readlink(path: &[u8], buf: u64, bufsize: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if buf == 0 && bufsize != 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    if kind != PATH_INDEX_KIND_SYMLINK {
        return Err(FsBridgeError::Invalid);
    }
    let target = snapshot_blob(&blob_id)?;
    if !is_valid_symlink_target(&target) {
        return Err(FsBridgeError::Invalid);
    }
    let copy_len = target.len().min(bufsize);
    if copy_len != 0 {
        copy_to_user(buf as *mut u8, target.as_ptr(), copy_len)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(copy_len as i64)
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
#[inline]
pub fn fs_readlinkat(
    dirfd: i32,
    path: &[u8],
    buf: u64,
    bufsize: usize,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_readlink(path, buf, bufsize, pid);
    }
    Err(FsBridgeError::Invalid)
}
