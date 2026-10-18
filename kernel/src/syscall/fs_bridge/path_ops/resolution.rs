//! # syscall/fs_bridge/path_ops/resolution.rs — Résolution, normalisation et indexation des chemins

use super::super::*;
use alloc::vec::Vec;
use core::mem::size_of;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ObjectId};
use crate::fs::exofs::path::path_component::{PathComponent, PathComponentBuf};
use crate::fs::exofs::path::path_index::{
    PathIndex, PathIndexHeader, PATH_INDEX_MAGIC, PATH_INDEX_VERSION,
};
use crate::fs::exofs::path::symlink::{is_valid_symlink_target, SYMLINK_MAX_DEPTH};
use crate::fs::exofs::syscall::object_store;

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
    if let Some(cached) = BLOB_CACHE.get(&blob_id) {
        if !blob_is_directory(&cached) {
            return Err(FsBridgeError::NotDir);
        }
        return PathIndex::from_bytes(&cached).map_err(exofs_to_bridge_error);
    }
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
    if blob_is_directory_by_id(&blob_id) {
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
    let header_len = size_of::<PathIndexHeader>();
    if let Ok(prefix) = BLOB_CACHE.read_at(blob_id, 0, header_len) {
        if prefix.len() == header_len {
            return blob_is_directory(&prefix);
        }
    }
    if let Ok(Some(data)) = object_store::load_blob_data_if_available(blob_id) {
        let is_dir = blob_is_directory(&data);
        let _ = BLOB_CACHE.insert(*blob_id, data);
        return is_dir;
    }
    false
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
