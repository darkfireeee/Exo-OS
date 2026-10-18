//! # syscall/fs_bridge/path_ops.rs

#![allow(unused_imports)]
use super::*;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use spin::Mutex as SpinMutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ExofsError, ObjectId};
use crate::fs::exofs::path::path_component::{PathComponent, PathComponentBuf};
use crate::fs::exofs::path::path_index::{
    PathIndex, PathIndexHeader, PATH_INDEX_MAGIC, PATH_INDEX_VERSION,
};
use crate::fs::exofs::path::symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, SYMLINK_MAX_DEPTH,
};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_store;
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};

#[inline]
pub fn validate_path(path: &[u8]) -> Result<(), FsBridgeError> {
    if path.is_empty() || path.len() > 4096 || path.contains(&0) {
        return Err(FsBridgeError::BadPath);
    }
    Ok(())
}

#[inline]
pub fn normalize_path_buf(path: &[u8]) -> Result<PathComponentBuf, FsBridgeError> {
    validate_path(path)?;
    let mut buf = PathComponentBuf::from_path(path).map_err(exofs_to_bridge_error)?;
    buf.normalize().map_err(exofs_to_bridge_error)?;
    Ok(buf)
}

#[inline]
pub fn path_buf_to_bytes(buf: &PathComponentBuf) -> Result<Vec<u8>, FsBridgeError> {
    buf.to_bytes().map_err(exofs_to_bridge_error)
}

#[inline]
pub fn normalized_path_bytes(path: &[u8]) -> Result<Vec<u8>, FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    path_buf_to_bytes(&buf)
}

#[inline]
pub fn split_parent_and_leaf(path: &[u8]) -> Result<(Vec<u8>, PathComponent), FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    let leaf = buf.last().cloned().ok_or(FsBridgeError::BadPath)?;
    let mut parent_buf = PathComponentBuf::new();
    for comp in buf.parent() {
        parent_buf
            .push(comp.clone())
            .map_err(exofs_to_bridge_error)?;
    }
    let parent_path = path_buf_to_bytes(&parent_buf)?;
    Ok((parent_path, leaf))
}

#[inline]
pub fn blob_id_for_path(path: &[u8]) -> Result<BlobId, FsBridgeError> {
    validate_path(path)?;
    Ok(BlobId::from_bytes_blake3(path))
}

#[inline]
pub fn blob_id_to_object_id(blob_id: BlobId) -> ObjectId {
    ObjectId(*blob_id.as_bytes())
}

#[inline]
pub fn object_id_to_blob_id(object_id: ObjectId) -> BlobId {
    BlobId(*object_id.as_bytes())
}

#[inline]
pub fn ensure_root_directory() -> Result<(), FsBridgeError> {
    let root_blob = blob_id_for_path(b"/")?;
    if BLOB_CACHE.contains(&root_blob) {
        return Ok(());
    }

    if let Some(data) =
        object_store::load_blob_data_if_available(&root_blob).map_err(exofs_to_bridge_error)?
    {
        if !blob_is_directory(&data) {
            return Err(FsBridgeError::NotDir);
        }
        return BLOB_CACHE
            .insert(root_blob, data)
            .map_err(exofs_to_bridge_error);
    }

    let dir_index = PathIndex::new_with_key(ObjectId::default(), directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(root_blob, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&root_blob);
    Ok(())
}

#[inline]
pub fn load_path_index(path: &[u8]) -> Result<PathIndex, FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }
    PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)
}

#[inline]
pub fn store_path_index(path: &[u8], index: &PathIndex) -> Result<(), FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let bytes = index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(())
}

#[inline]
pub fn ensure_directory_exists(path: &[u8]) -> Result<(), FsBridgeError> {
    if path == b"/" {
        return ensure_root_directory();
    }

    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if blob_is_directory(&data) {
        Ok(())
    } else {
        Err(FsBridgeError::NotDir)
    }
}

#[inline]
pub fn ensure_directory_chain(path: &[u8]) -> Result<(), FsBridgeError> {
    let normalized_path = normalized_path_bytes(path)?;
    ensure_root_directory()?;
    if normalized_path == b"/" {
        return Ok(());
    }

    let components = normalize_path_buf(&normalized_path)?;
    let mut current = PathComponentBuf::new();

    for comp in components.iter() {
        let parent_path = path_buf_to_bytes(&current)?;
        let next_comp = comp.clone();
        current
            .push(next_comp.clone())
            .map_err(exofs_to_bridge_error)?;
        let current_path = path_buf_to_bytes(&current)?;

        match path_entry(&current_path) {
            Ok((blob_id, kind)) => {
                if kind == PATH_INDEX_KIND_DIR {
                    continue;
                }

                let data = snapshot_blob(&blob_id)?;
                if !blob_is_directory(&data) {
                    return Err(FsBridgeError::NotDir);
                }
            }
            Err(FsBridgeError::NotFound) => {
                let blob_id = blob_id_for_path(&current_path)?;
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
                upsert_parent_entry(&parent_path, &next_comp, blob_id, PATH_INDEX_KIND_DIR)?;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

#[inline]
pub fn upsert_parent_entry(
    parent_path: &[u8],
    leaf: &PathComponent,
    child_blob_id: BlobId,
    kind: u8,
) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    if index.lookup(leaf).is_some() {
        index.remove(leaf).map_err(exofs_to_bridge_error)?;
    }
    index
        .insert(leaf, blob_id_to_object_id(child_blob_id), kind)
        .map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
pub fn remove_parent_entry(parent_path: &[u8], leaf: &PathComponent) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    index.remove(leaf).map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
pub fn directory_mount_key() -> [u8; 16] {
    #[cfg(target_os = "none")]
    {
        crate::fs::exofs::path::path_index::mount_secret_key()
    }

    #[cfg(not(target_os = "none"))]
    {
        [0xA5, 0, 0, 0, 0, 0, 0, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0xC3]
    }
}

#[inline]
pub fn path_index_entry_count(data: &[u8]) -> Option<u32> {
    if data.len() < size_of::<PathIndexHeader>() {
        return None;
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != PATH_INDEX_MAGIC {
        return None;
    }

    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != PATH_INDEX_VERSION {
        return None;
    }

    Some(u32::from_le_bytes([data[40], data[41], data[42], data[43]]))
}

#[inline]
pub fn blob_is_directory(data: &[u8]) -> bool {
    path_index_entry_count(data).is_some()
}

#[inline]
pub fn blob_is_directory_by_id(blob_id: &BlobId) -> bool {
    snapshot_blob(blob_id)
        .ok()
        .as_deref()
        .and_then(path_index_entry_count)
        .is_some()
}

#[inline]
pub fn path_entry(path: &[u8]) -> Result<(BlobId, u8), FsBridgeError> {
    ensure_root_directory()?;
    if path == b"/" {
        return Ok((blob_id_for_path(path)?, PATH_INDEX_KIND_DIR));
    }
    let (parent_path, leaf) = split_parent_and_leaf(path)?;
    let index = load_path_index(&parent_path)?;
    let (object_id, kind) = index.lookup(&leaf).ok_or(FsBridgeError::NotFound)?;
    Ok((object_id_to_blob_id(object_id), kind))
}

pub fn resolve_path_with_symlinks(
    path: &[u8],
    follow_last: bool,
    allow_missing_final: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let mut pending = normalize_path_buf(path)?;
    let mut resolved = PathComponentBuf::new();
    let mut depth = 0usize;
    let mut idx = 0usize;

    ensure_root_directory()?;

    while idx < pending.len() {
        let comp = pending.as_slice()[idx].clone();
        let parent_path = path_buf_to_bytes(&resolved)?;
        ensure_directory_exists(&parent_path)?;

        let index = load_path_index(&parent_path)?;
        let (object_id, kind) = match index.lookup(&comp) {
            Some(entry) => entry,
            None => {
                let is_last = idx + 1 == pending.len();
                if !allow_missing_final || !is_last {
                    return Err(FsBridgeError::NotFound);
                }
                resolved.push(comp).map_err(exofs_to_bridge_error)?;
                return path_buf_to_bytes(&resolved);
            }
        };

        let is_last = idx + 1 == pending.len();
        if kind == PATH_INDEX_KIND_SYMLINK && (!is_last || follow_last) {
            depth = depth.saturating_add(1);
            if depth > SYMLINK_MAX_DEPTH {
                return Err(FsBridgeError::Loop);
            }

            let raw_target = snapshot_blob(&object_id_to_blob_id(object_id))?;
            if !is_valid_symlink_target(&raw_target) {
                return Err(FsBridgeError::Invalid);
            }

            let mut next = if raw_target.starts_with(b"/") {
                normalize_path_buf(&raw_target)?
            } else {
                let mut joined = resolved.clone();
                let target_buf =
                    PathComponentBuf::from_path(&raw_target).map_err(exofs_to_bridge_error)?;
                joined
                    .extend_from(&target_buf)
                    .map_err(exofs_to_bridge_error)?;
                joined
            };

            let mut rem_idx = idx + 1;
            while rem_idx < pending.len() {
                next.push(pending.as_slice()[rem_idx].clone())
                    .map_err(exofs_to_bridge_error)?;
                rem_idx += 1;
            }
            next.normalize().map_err(exofs_to_bridge_error)?;
            pending = next;
            resolved = PathComponentBuf::new();
            idx = 0;
            continue;
        }

        resolved.push(comp).map_err(exofs_to_bridge_error)?;
        idx += 1;
    }

    path_buf_to_bytes(&resolved)
}

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
    // ExoFS ne maintient pas encore de namespace de répertoires ouverts.
    // Cas supportés sans ambiguïté:
    // - AT_FDCWD + chemin relatif/absolu
    // - chemin absolu, quel que soit dirfd (sémantique POSIX)
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_open(path, flags, mode, pid);
    }
    Err(FsBridgeError::Invalid)
}

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

/// `getdents64(fd, dirp, count)`.
#[inline]
pub fn fs_getdents64(fd: u32, dirp: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    if dirp == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count < DIRENT64_HEADER_SIZE + 2 {
        return Err(FsBridgeError::Invalid);
    }

    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let data = snapshot_blob(&entry.blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }

    let index = PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)?;
    let entries = index.entries();
    let start_idx = entry.cursor as usize;
    if start_idx >= entries.len() {
        return Ok(0);
    }

    let mut cursor = start_idx;
    let mut written = 0usize;
    while cursor < entries.len() {
        let entry_ref = &entries[cursor];
        let name = entry_ref.name_bytes();
        let raw_size = DIRENT64_HEADER_SIZE
            .checked_add(name.len())
            .and_then(|v| v.checked_add(1))
            .ok_or(FsBridgeError::NoSpace)?;
        let reclen = (raw_size + 7) & !7usize;
        if written.saturating_add(reclen) > count {
            break;
        }

        let header = LinuxDirent64 {
            d_ino: inode_from_object_id(&entry_ref.oid),
            d_off: (cursor + 1) as i64,
            d_reclen: reclen as u16,
            d_type: dirent_type_from_kind(entry_ref.kind),
        };

        // SAFETY: LinuxDirent64 est #[repr(C)] et POD.
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const LinuxDirent64 as *const u8,
                DIRENT64_HEADER_SIZE,
            )
        };

        let base = dirp.saturating_add(written as u64);
        copy_to_user(base as *mut u8, header_bytes.as_ptr(), header_bytes.len())
            .map_err(|_| FsBridgeError::Fault)?;
        copy_to_user(
            base.saturating_add(DIRENT64_HEADER_SIZE as u64) as *mut u8,
            name.as_ptr(),
            name.len(),
        )
        .map_err(|_| FsBridgeError::Fault)?;

        let zero = [0u8; 8];
        copy_to_user(
            base.saturating_add((DIRENT64_HEADER_SIZE + name.len()) as u64) as *mut u8,
            zero.as_ptr(),
            1,
        )
        .map_err(|_| FsBridgeError::Fault)?;

        let pad = reclen.saturating_sub(raw_size);
        if pad != 0 {
            copy_to_user(
                base.saturating_add(raw_size as u64) as *mut u8,
                zero.as_ptr(),
                pad,
            )
            .map_err(|_| FsBridgeError::Fault)?;
        }

        written = written.saturating_add(reclen);
        cursor += 1;
    }

    if written == 0 {
        return Ok(0);
    }

    OBJECT_TABLE
        .set_cursor(obj_fd, cursor as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
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

/// `access(path, mode)`.
#[inline]
pub fn fs_access(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if mode & !0x7 != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let _ = path_entry(&normalized_path)?;
    Ok(0)
}

/// `statfs(path, buf)`.
#[inline]
pub fn fs_statfs(path: &[u8], statfs_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if statfs_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let _ = path_entry(&normalized_path)?;
    write_user_typed(statfs_ptr, statfs_snapshot()).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `fstatfs(fd, buf)`.
#[inline]
pub fn fs_fstatfs(fd: u32, statfs_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if statfs_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    write_user_typed(statfs_ptr, statfs_snapshot()).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// Metadata permission calls are accepted once the target exists; ExoFS uses
/// capabilities rather than Unix ownership bits for enforcement.
#[inline]
pub fn fs_chmod(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let is_dir = kind == PATH_INDEX_KIND_DIR;
    let type_bits = default_stat_mode_for_kind(kind, is_dir) & S_IFMT;
    upsert_mode(blob_id, type_bits | (mode & 0o7777));
    Ok(0)
}

#[inline]
pub fn fs_fchmod(fd: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    let is_dir = blob_is_directory_by_id(&entry.blob_id);
    let type_bits = if is_dir { S_IFDIR } else { S_IFREG };
    upsert_mode(entry.blob_id, type_bits | (mode & 0o7777));
    Ok(0)
}

#[inline]
pub fn fs_chown(path: &[u8], uid: u32, gid: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (uid, gid, pid);
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let _ = path_entry(&normalized_path)?;
    Ok(0)
}

#[inline]
pub fn fs_fchown(fd: u32, uid: u32, gid: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (uid, gid);
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `mknod(path, mode, dev)` for regular files/FIFOs in the ExoFS namespace.
#[inline]
pub fn fs_mknod(path: &[u8], mode: u32, dev: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (dev, pid);
    let kind = mode & S_IFMT;
    if kind != 0 && kind != S_IFREG && kind != S_IFIFO {
        return Err(FsBridgeError::Invalid);
    }
    let fd = fs_open(
        path,
        open_flags::O_CREAT | open_flags::O_EXCL | open_flags::O_RDWR,
        mode,
        pid,
    )? as u32;
    let _ = fs_close(fd, pid);
    Ok(0)
}

/// `mknodat(dirfd, path, mode, dev)`.
#[inline]
pub fn fs_mknodat(
    dirfd: i32,
    path: &[u8],
    mode: u32,
    dev: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_mknod(path, mode, dev, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `sync_file_range(fd, offset, nbytes, flags)`.
#[inline]
pub fn fs_sync_file_range(
    fd: u32,
    offset: u64,
    nbytes: u64,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (offset, nbytes, pid);
    let allowed = SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER;
    if flags & !allowed != 0 {
        return Err(FsBridgeError::Invalid);
    }
    fs_fsync(fd, false, pid)
}

/// `sync()`.
#[inline]
pub fn fs_sync(pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;

    if BLOB_CACHE.collect_dirty().is_empty() {
        return Ok(0);
    }

    // FIX-EXOFS-CORE-1 (AUDIT-EXOFS §2) : `sync()` déclenche un commit d'epoch
    // transactionnel (durabilité atomique données + métadonnées) au lieu d'une
    // persistance brute blob par blob. Le commit flush lui-même tous les blobs
    // dirty puis scelle l'epoch (journal + EpochRoot/Record + barrières NVMe).
    match crate::fs::exofs::syscall::epoch_commit::commit_current_epoch() {
        Ok(_) | Err(ExofsError::CommitInProgress) => Ok(0),
        Err(_) => Err(FsBridgeError::Io),
    }
}

/// `statx(dirfd, path, flags, mask, statxbuf)`.
#[inline]
pub fn fs_statx(
    dirfd: i32,
    path: &[u8],
    flags: u32,
    mask: u32,
    statx_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    pub const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    pub const AT_EMPTY_PATH: u32 = 0x1000;
    let _ = mask;
    if statx_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return Err(FsBridgeError::Invalid);
    }
    if dirfd != AT_FDCWD && !path.starts_with(b"/") {
        return Err(FsBridgeError::Invalid);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let normalized_path = resolve_path_with_symlinks(path, follow, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let statx = linux_statx_for_blob_meta(
        blob_id,
        blob_len(&blob_id) as u64,
        pid,
        kind,
        kind == PATH_INDEX_KIND_DIR,
    );
    write_user_typed(statx_ptr, statx).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

