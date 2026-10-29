//! # syscall/fs_bridge/path_ops/operations.rs — Opérations VFS / POSIX sur les fichiers et dossiers

use super::super::*;
use super::resolution::*;
use alloc::vec::Vec;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::ObjectId;
use crate::fs::exofs::path::path_index::PathIndex;
use crate::fs::exofs::path::symlink::invalidate_symlink;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::validation::write_user_typed;

/// `open(path, flags, mode)` → fd.
#[inline]
pub fn fs_open(path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if is_tty_path(path) {
        let fd_flags = fd_table_flags(flags, open_flags::O_RDWR);
        if let Some(logical_fd) = install_process_fd(pid, TTY_PTS0_HANDLE as u64, fd_flags) {
            return Ok(logical_fd as i64);
        }
        return Ok(TTY_PTS0_HANDLE as i64);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let fd_flags = flags & !(O_CLOEXEC | O_NONBLOCK);
    if !open_flags::validate(fd_flags) {
        return Err(FsBridgeError::Invalid);
    }

    ensure_root_directory()?;
    let normalized_input = normalized_path_bytes(path)?;
    if fd_flags & open_flags::O_CREAT != 0 && normalized_input != b"/" {
        let (parent_path, _) = split_parent_and_leaf(&normalized_input)?;
        ensure_directory_chain(&parent_path)?;
    }
    let normalized_path = resolve_path_with_symlinks(path, true, true)?;
    let existing_entry = path_entry(&normalized_path).ok();
    let blob_id = match existing_entry {
        Some((existing_blob_id, _)) => existing_blob_id,
        None => blob_id_for_path(&normalized_path)?,
    };
    let exists = existing_entry.is_some();

    if !exists && fd_flags & open_flags::O_CREAT == 0 {
        return Err(FsBridgeError::NotFound);
    }
    if exists && (fd_flags & open_flags::O_CREAT != 0) && (fd_flags & open_flags::O_EXCL != 0) {
        return Err(FsBridgeError::Exists);
    }
    if !exists {
        let effective_mode = S_IFREG | apply_umask(mode, 0o666, pid);
        let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
        ensure_directory_exists(&parent_path)?;
        BLOB_CACHE
            .insert(blob_id, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
        upsert_mode(blob_id, effective_mode);
        upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_FILE)?;
    }
    if exists {
        ensure_blob_exists(blob_id)?;
    }
    if fd_flags & open_flags::O_TRUNC != 0 {
        if !open_flags::can_write(fd_flags) {
            return Err(FsBridgeError::Invalid);
        }
        if existing_entry
            .map(|(_, kind)| kind == PATH_INDEX_KIND_DIR)
            .unwrap_or(false)
        {
            return Err(FsBridgeError::IsDir);
        }
        BLOB_CACHE
            .insert(blob_id, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }

    if existing_entry.map(|(_, kind)| kind == PATH_INDEX_KIND_DIR).unwrap_or(false) {
        crate::syscall::fs_bridge::cwd::register_dir_path(blob_id, &normalized_path);
    }

    let size = blob_len(&blob_id) as u64;
    let fd = OBJECT_TABLE
        .open(blob_id, fd_flags, size, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if fd_flags & open_flags::O_APPEND != 0 {
        OBJECT_TABLE
            .set_cursor(fd, size)
            .map_err(exofs_to_bridge_error)?;
    }
    if let Some(logical_fd) = install_process_fd(pid, fd as u64, fd_table_flags(flags, fd_flags)) {
        Ok(logical_fd as i64)
    } else {
        Ok(fd as i64)
    }
}

/// `stat(path, stat_ptr)`.
#[inline]
pub fn fs_stat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let size = blob_len(&blob_id) as u64;
    let is_dir = kind == PATH_INDEX_KIND_DIR;
    let stat = linux_stat_for_blob_meta(blob_id, size, pid, kind, is_dir);
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `lstat(path, stat_ptr)` — ne suit pas le symlink terminal.
#[inline]
pub fn fs_lstat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let stat = linux_stat_for_blob_meta(
        blob_id,
        blob_len(&blob_id) as u64,
        pid,
        kind,
        kind == PATH_INDEX_KIND_DIR,
    );
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `fstat(fd, stat_ptr)`.
#[inline]
pub fn fs_fstat(fd: u32, stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let owner_uid = if entry.owner_uid == 0 {
        pid
    } else {
        entry.owner_uid as u32
    };
    let is_dir = blob_is_directory_by_id(&entry.blob_id);
    let kind = if is_dir {
        PATH_INDEX_KIND_DIR
    } else {
        PATH_INDEX_KIND_FILE
    };
    let stat = linux_stat_for_blob_meta(
        entry.blob_id,
        blob_len(&entry.blob_id) as u64,
        owner_uid,
        kind,
        is_dir,
    );
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `openat(dirfd, path, flags, mode)`.
#[inline]
pub fn fs_openat(
    dirfd: i32,
    path: &[u8],
    flags: u32,
    mode: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    // Cas supportés sans ambiguïté:
    // - AT_FDCWD + chemin relatif/absolu
    // - chemin absolu, quel que soit dirfd (sémantique POSIX)
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_open(path, flags, mode, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `mkdir(path, mode)`.
#[inline(never)]
pub fn fs_mkdir(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        ensure_root_directory()?;
        return Ok(0);
    }
    ensure_root_directory()?;
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    ensure_directory_chain(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    if BLOB_CACHE.contains(&blob_id) {
        return Err(FsBridgeError::Exists);
    }
    let effective_mode = S_IFDIR | apply_umask(mode, 0o777, pid);

    let parent_oid = if parent_path == b"/" {
        ObjectId::default()
    } else {
        blob_id_to_object_id(blob_id_for_path(&parent_path)?)
    };
    let dir_index = PathIndex::new_with_key(parent_oid, directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    upsert_mode(blob_id, effective_mode);
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_DIR)?;
    crate::syscall::fs_bridge::cwd::register_dir_path(blob_id, &normalized_path);
    Ok(0)
}

/// `rmdir(path)`.
#[inline]
pub fn fs_rmdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    let entry_count = path_index_entry_count(&data).ok_or(FsBridgeError::NotDir)?;
    if entry_count != 0 {
        return Err(FsBridgeError::NotEmpty);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    remove_mode(&blob_id);
    Ok(0)
}

/// `unlink(path)`.
#[inline]
pub fn fs_unlink(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    if kind == PATH_INDEX_KIND_DIR || blob_is_directory(&data) {
        return Err(FsBridgeError::IsDir);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    if kind == PATH_INDEX_KIND_SYMLINK {
        invalidate_symlink(&blob_id_to_object_id(blob_id));
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    remove_mode(&blob_id);
    Ok(0)
}

/// `rename(oldpath, newpath)`.
#[inline]
pub fn fs_rename(old_path: &[u8], new_path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let old_normalized = resolve_path_with_symlinks(old_path, false, false)?;
    let new_normalized = resolve_path_with_symlinks(new_path, false, true)?;
    if old_normalized == new_normalized {
        return Ok(0);
    }
    if old_normalized == b"/" || new_normalized == b"/" {
        return Err(FsBridgeError::PermDenied);
    }

    let (old_parent, old_leaf) = split_parent_and_leaf(&old_normalized)?;
    let (new_parent, new_leaf) = split_parent_and_leaf(&new_normalized)?;
    ensure_directory_exists(&new_parent)?;

    let (src_blob_id, src_kind) = path_entry(&old_normalized)?;
    let src_data = snapshot_blob(&src_blob_id)?;
    let src_is_dir = src_kind == PATH_INDEX_KIND_DIR || blob_is_directory(&src_data);
    if src_is_dir
        && new_normalized.len() > old_normalized.len()
        && new_normalized.starts_with(&old_normalized)
        && new_normalized[old_normalized.len()] == b'/'
    {
        return Err(FsBridgeError::Invalid);
    }

    match path_entry(&new_normalized) {
        Ok((dst_blob_id, dst_kind)) => {
            let dst_data = snapshot_blob(&dst_blob_id)?;
            let dst_is_dir = dst_kind == PATH_INDEX_KIND_DIR || blob_is_directory(&dst_data);
            if src_is_dir != dst_is_dir {
                return if src_is_dir {
                    Err(FsBridgeError::NotDir)
                } else {
                    Err(FsBridgeError::IsDir)
                };
            }
            if dst_is_dir && path_index_entry_count(&dst_data).unwrap_or(0) != 0 {
                return Err(FsBridgeError::NotEmpty);
            }
            if OBJECT_TABLE.open_count_for(&dst_blob_id) != 0 {
                return Err(FsBridgeError::PermDenied);
            }
            remove_parent_entry(&new_parent, &new_leaf)?;
            BLOB_CACHE.invalidate(&dst_blob_id);
            remove_mode(&dst_blob_id);
        }
        Err(FsBridgeError::NotFound) => {}
        Err(err) => return Err(err),
    }

    upsert_parent_entry(&new_parent, &new_leaf, src_blob_id, src_kind)?;
    remove_parent_entry(&old_parent, &old_leaf)?;
    let _ = BLOB_CACHE.mark_dirty(&src_blob_id);
    Ok(0)
}

/// `renameat(olddirfd, oldpath, newdirfd, newpath)`.
#[inline]
pub fn fs_renameat(
    olddirfd: i32,
    old_path: &[u8],
    newdirfd: i32,
    new_path: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if olddirfd == AT_FDCWD || old_path.starts_with(b"/") {
        if newdirfd == AT_FDCWD || new_path.starts_with(b"/") {
            return fs_rename(old_path, new_path, pid);
        }
    }
    Err(FsBridgeError::Invalid)
}

/// `link(oldpath, newpath)`.
#[inline]
pub fn fs_link(old_path: &[u8], new_path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    fs_link_with_follow(old_path, new_path, true, pid)
}

/// `linkat(olddirfd, oldpath, newdirfd, newpath, flags)`.
#[inline]
pub fn fs_linkat(
    olddirfd: i32,
    old_path: &[u8],
    newdirfd: i32,
    new_path: &[u8],
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    pub const AT_SYMLINK_FOLLOW: u32 = 0x400;
    if olddirfd != AT_FDCWD && !old_path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    if newdirfd != AT_FDCWD && !new_path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    if flags & !AT_SYMLINK_FOLLOW != 0 {
        return Err(FsBridgeError::Invalid);
    }
    fs_link_with_follow(old_path, new_path, flags & AT_SYMLINK_FOLLOW != 0, pid)
}
