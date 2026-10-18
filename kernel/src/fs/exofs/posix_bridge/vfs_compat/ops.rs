#![allow(unused_imports, dead_code)]
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::object_write::{persist_cached_blob_if_disk, reject_if_immutable};
use crate::fs::exofs::syscall::quota_query::{check_quota, quota_add_usage, quota_sub_usage};

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Opérations VFS — surface POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Résout `name` dans le répertoire `parent_ino`. Retourne l'ino fils.
pub fn vfs_lookup(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    // Vérifie que le parent est un répertoire.
    let parent = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    DIRECTORY_REGISTRY
        .lookup(parent_ino, name)
        .map(|record| record.ino)
        .ok_or(ExofsError::ObjectNotFound)
}

/// Crée le mapping d'un fichier sous `parent_ino`. Retourne l'ino créé.
pub fn vfs_create(
    parent_ino: ObjectIno,
    name: &[u8],
    mode: u32,
    uid: u64,
) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let parent = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let oid = hash_name(parent_ino, name);
    // Vérifie qu'il n'existe pas déjà.
    if INODE_EMULATION.contains_oid(oid) {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    check_quota(uid, 0, 1)?;
    let flags = if mode & file_mode::S_IFMT == file_mode::S_IFDIR {
        inode_flags::DIRECTORY
    } else {
        inode_flags::REGULAR
    };
    let ino = INODE_EMULATION.get_or_alloc_flags(oid, flags, 0, uid)?;
    if let Err(err) = DIRECTORY_REGISTRY.insert(parent_ino, ino, name, inode_kind(flags)) {
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    if let Err(err) = quota_add_usage(uid, 0, 1) {
        let _ = DIRECTORY_REGISTRY.remove(parent_ino, name);
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    if let Err(err) = persist_vfs_namespace_if_disk() {
        let _ = DIRECTORY_REGISTRY.remove(parent_ino, name);
        INODE_EMULATION.release_ino(ino);
        let _ = quota_sub_usage(uid, 0, 1);
        return Err(err);
    }
    Ok(ino)
}

/// Ouvre un inode et retourne un fd.
pub fn vfs_open(ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
    let entry = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if flags & open_flags::O_WRONLY != 0 || flags & open_flags::O_RDWR != 0 {
        if entry.flags & inode_flags::READ_ONLY != 0 {
            return Err(ExofsError::PermissionDenied);
        }
    }
    if flags & open_flags::O_TRUNC != 0 {
        if entry.flags & inode_flags::DIRECTORY != 0 {
            return Err(ExofsError::WrongObjectKind);
        }
        resize_inode_data(&entry, 0)?;
        INODE_EMULATION.update_size(ino, 0)?;
        let _ = persist_vfs_namespace_if_disk()?;
    }
    FD_TABLE.open_fd(ino, flags, pid)
}

/// Ferme un fd.
pub fn vfs_close(fd: u64) -> ExofsResult<()> {
    FD_TABLE.close_fd(fd)
}

pub fn vfs_read(fd: u64, buf: &mut [u8], count: usize) -> ExofsResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY != 0 {
        return Err(ExofsError::PermissionDenied);
    }
    let entry = INODE_EMULATION
        .get_entry(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    let blob_id = ensure_inode_blob_cached(&entry)?;
    let start = desc.offset as usize;
    let current_len = BLOB_CACHE.len(&blob_id).unwrap_or(0);
    if start >= current_len {
        return Ok(0);
    }
    let readable = count.min(buf.len()).min(current_len.saturating_sub(start));
    if readable == 0 {
        return Ok(0);
    }
    let data = BLOB_CACHE.read_at(&blob_id, start, readable)?;
    buf[..readable].copy_from_slice(&data);
    let new_offset = desc.offset.saturating_add(readable as u64);
    FD_TABLE.update_offset(fd, new_offset);
    Ok(readable)
}

/// Écrit des données vers un fd.
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY == 0 && desc.flags & open_flags::O_RDWR == 0 {
        return Err(ExofsError::PermissionDenied);
    }
    let entry = INODE_EMULATION
        .get_entry(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    let written = count.min(buf.len());
    let blob_id = ensure_inode_blob_cached(&entry)?;
    reject_if_immutable(blob_id)?;
    let current_len = BLOB_CACHE.len(&blob_id).unwrap_or(0);
    let start_offset = if desc.flags & open_flags::O_APPEND != 0 {
        current_len
    } else {
        desc.offset as usize
    };
    let end_offset = start_offset
        .checked_add(written)
        .ok_or(ExofsError::OffsetOverflow)?;
    let new_size = current_len.max(end_offset) as u64;
    let growth = new_size.saturating_sub(current_len as u64);
    if growth > 0 {
        check_quota(entry.uid, growth, 0)?;
    }
    BLOB_CACHE.write_at(blob_id, start_offset, &buf[..written])?;
    persist_cached_blob_if_disk(blob_id)?;
    if growth > 0 {
        quota_add_usage(entry.uid, growth, 0)?;
    }
    let new_offset = end_offset as u64;
    FD_TABLE.update_offset(fd, new_offset);
    if new_size != entry.size {
        INODE_EMULATION.update_size(desc.ino, new_size)?;
        let _ = persist_vfs_namespace_if_disk()?;
    }
    Ok(written)
}

/// Retourne les métadonnées d'un inode.
pub fn vfs_getattr(ino: ObjectIno) -> ExofsResult<VfsInode> {
    let e = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    Ok(entry_to_vfs_inode(&e))
}

/// Crée un répertoire.
pub fn vfs_mkdir(
    parent_ino: ObjectIno,
    name: &[u8],
    mode: u32,
    uid: u64,
) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let dir_mode = (mode & !file_mode::S_IFMT) | file_mode::S_IFDIR;
    vfs_create(parent_ino, name, dir_mode, uid)
}

/// Supprime un fichier du répertoire parent.
pub fn vfs_unlink(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let record = DIRECTORY_REGISTRY
        .lookup(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION
        .get_entry(record.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    // Ne peut pas unlink un répertoire.
    if entry.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::NotADirectory);
    }
    let blob_id = blob_id_for_object(entry.object_id);
    reject_if_immutable(blob_id)?;
    let removed = DIRECTORY_REGISTRY
        .remove(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    if let Err(err) = object_store::free_blob_mapping(&blob_id) {
        let _ = DIRECTORY_REGISTRY.insert(parent_ino, removed.ino, &removed.name, removed.kind);
        return Err(err);
    }
    BLOB_CACHE.invalidate(&blob_id);
    quota_sub_usage(entry.uid, entry.size, 1)?;
    INODE_EMULATION.release_ino(record.ino);
    let _ = persist_vfs_namespace_if_disk()?;
    Ok(())
}

/// Supprime un répertoire (doit être vide).
pub fn vfs_rmdir(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let record = DIRECTORY_REGISTRY
        .lookup(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION
        .get_entry(record.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    if DIRECTORY_REGISTRY.has_children(record.ino) {
        return Err(ExofsError::DirectoryNotEmpty);
    }
    DIRECTORY_REGISTRY
        .remove(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    DIRECTORY_REGISTRY.remove_dir(record.ino);
    INODE_EMULATION.release_ino(record.ino);
    let _ = persist_vfs_namespace_if_disk()?;
    Ok(())
}

/// Renomme `old_name` de `old_parent` vers `new_parent/new_name`.
pub fn vfs_rename(
    old_parent: ObjectIno,
    old_name: &[u8],
    new_parent: ObjectIno,
    new_name: &[u8],
) -> ExofsResult<()> {
    if old_name.is_empty() || new_name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if new_name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(old_name)?;
    validate_name(new_name)?;
    // Vérifie que les deux parents sont des répertoires.
    let op = INODE_EMULATION
        .get_entry(old_parent)
        .ok_or(ExofsError::ObjectNotFound)?;
    let np = INODE_EMULATION
        .get_entry(new_parent)
        .ok_or(ExofsError::ObjectNotFound)?;
    if op.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    if np.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let src = DIRECTORY_REGISTRY
        .lookup(old_parent, old_name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let _ = DIRECTORY_REGISTRY.rename(old_parent, old_name, new_parent, new_name)?;
    let _ = (op, np, src);
    let _ = persist_vfs_namespace_if_disk()?;
    Ok(())
}

/// Retourne un vecteur de VfsDirent pour un ino répertoire.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn vfs_readdir(parent_ino: ObjectIno, _offset: u64) -> ExofsResult<Vec<VfsDirent>> {
    let entry = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let records = DIRECTORY_REGISTRY.list(parent_ino)?;
    let mut out: Vec<VfsDirent> = Vec::new();
    out.try_reserve(records.len().saturating_add(2))
        .map_err(|_| ExofsError::NoMemory)?;
    // Toujours inclure "." et ".."
    let dot = make_dirent(parent_ino, b".", 4);
    let dotdot = make_dirent(VFS_ROOT_INO, b"..", 4);
    out.push(dot);
    out.push(dotdot);
    let mut i = 0usize;
    while i < records.len() {
        let record = &records[i];
        out.push(make_dirent(record.ino, &record.name, record.kind));
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Tronque/étend un fichier à `new_size`.
pub fn vfs_truncate(ino: ObjectIno, new_size: u64) -> ExofsResult<()> {
    let e = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if e.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    if new_size > usize::MAX as u64 {
        return Err(ExofsError::OffsetOverflow);
    }
    let new_len = new_size as usize;
    resize_inode_data(&e, new_len)?;
    INODE_EMULATION.update_size(ino, new_size)?;
    let _ = persist_vfs_namespace_if_disk()?;
    Ok(())
}

/// Crée un lien symbolique.
pub fn vfs_symlink(parent_ino: ObjectIno, name: &[u8], uid: u64) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let oid = hash_name(parent_ino, name);
    if INODE_EMULATION.contains_oid(oid) {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    let ino = INODE_EMULATION.get_or_alloc_flags(oid, inode_flags::SYMLINK, 0, uid)?;
    if let Err(err) =
        DIRECTORY_REGISTRY.insert(parent_ino, ino, name, inode_kind(inode_flags::SYMLINK))
    {
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    if let Err(err) = persist_vfs_namespace_if_disk() {
        let _ = DIRECTORY_REGISTRY.remove(parent_ino, name);
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    Ok(ino)
}

/// Ferme tous les fd d'un pid (exit/kill).
pub fn vfs_close_all_pid(pid: u32) {
    FD_TABLE.close_all_pid(pid);
}

/// Nombre de descripteurs ouverts.
pub fn vfs_open_count() -> usize {
    FD_TABLE.active_count()
}

#[cfg(test)]
pub fn reset_vfs_state_for_test() {
    FD_TABLE.reset_all();
    DIRECTORY_REGISTRY.reset_all();
    INODE_EMULATION.clear();
    BLOB_CACHE.flush_all_force();
    object_store::OBJECT_STORE.reset_all();
    VFS_REGISTERED.store(false, Ordering::Release);
}

