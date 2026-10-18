//! # syscall/fs_bridge/path_ops/metadata.rs — Lecture de répertoire, attributs, permissions et statx

use super::super::*;
use super::operations::fs_open;
use super::resolution::*;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::ExofsError;
use crate::fs::exofs::path::path_index::PathIndex;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::validation::{copy_to_user, write_user_typed};

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
    // persistance brute blob par blob.
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
