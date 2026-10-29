//! # syscall/fs_bridge/file_ops.rs

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
pub fn blob_len(blob_id: &BlobId) -> usize {
    BLOB_CACHE
        .len(blob_id)
        .or_else(|| object_store::persisted_size(blob_id).map(|len| len as usize))
        .unwrap_or(0)
}

#[inline]
pub fn ensure_blob_exists(blob_id: BlobId) -> Result<(), FsBridgeError> {
    if BLOB_CACHE.contains(&blob_id) {
        return Ok(());
    }
    if let Some(data) =
        object_store::load_blob_data_if_available(&blob_id).map_err(exofs_to_bridge_error)?
    {
        return BLOB_CACHE
            .insert(blob_id, data)
            .map_err(exofs_to_bridge_error);
    }
    BLOB_CACHE
        .insert(blob_id, Vec::new())
        .map_err(exofs_to_bridge_error)
}

#[inline]
pub fn snapshot_blob(blob_id: &BlobId) -> Result<Vec<u8>, FsBridgeError> {
    if let Some(data) = BLOB_CACHE.get(blob_id) {
        return Ok(data.to_vec());
    }

    if let Some(data) =
        object_store::load_blob_data_if_available(blob_id).map_err(exofs_to_bridge_error)?
    {
        BLOB_CACHE
            .insert(*blob_id, data.clone())
            .map_err(exofs_to_bridge_error)?;
        return Ok(data);
    }

    Err(FsBridgeError::NotFound)
}

#[inline]
pub fn resize_regular_blob(blob_id: BlobId, length: u64) -> Result<(), FsBridgeError> {
    if length > i64::MAX as u64 || length > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    ensure_blob_exists(blob_id)?;
    if blob_is_directory_by_id(&blob_id) {
        return Err(FsBridgeError::IsDir);
    }

    BLOB_CACHE
        .resize(blob_id, length as usize)
        .map_err(exofs_to_bridge_error)?;
    Ok(())
}

#[inline]
pub fn read_blob_at(
    blob_id: BlobId,
    offset: u64,
    buf_ptr: u64,
    count: usize,
) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 && count != 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }

    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let data = BLOB_CACHE
        .read_at(&blob_id, offset as usize, count)
        .map_err(exofs_to_bridge_error)?;
    if data.is_empty() {
        return Ok(0);
    }
    copy_to_user(buf_ptr as *mut u8, data.as_ptr(), data.len())
        .map_err(|_| FsBridgeError::Fault)?;
    Ok(data.len() as i64)
}

#[inline]
pub fn write_blob_at(
    blob_id: BlobId,
    offset: u64,
    buf_ptr: u64,
    count: usize,
) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 && count != 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let input = read_user_bytes(buf_ptr, count)?;

    let start = offset as usize;
    BLOB_CACHE
        .write_at(blob_id, start, &input)
        .map_err(exofs_to_bridge_error)?;
    Ok(count as i64)
}

#[inline]
pub fn read_blob_bytes_at(
    blob_id: BlobId,
    offset: u64,
    count: usize,
) -> Result<Vec<u8>, FsBridgeError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }
    BLOB_CACHE
        .read_at(&blob_id, offset as usize, count)
        .map_err(exofs_to_bridge_error)
}

#[inline]
pub fn write_blob_bytes_at(blob_id: BlobId, offset: u64, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    if offset > usize::MAX as u64 {
        return Err(FsBridgeError::Invalid);
    }

    let start = offset as usize;
    BLOB_CACHE
        .write_at(blob_id, start, bytes)
        .map_err(exofs_to_bridge_error)?;
    Ok(bytes.len() as i64)
}

/// `read(fd, buf, count)` → octets lus.
/// PONT : `crate::fs::vfs::read(fd, buf_slice)` — activé quand `pub mod fs;`
#[inline]
pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        if !fd_can_read_flags(resolved.flags) {
            return Err(FsBridgeError::BadFd);
        }
        return tty_read_bytes(buf_ptr, count, pid);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }

    let obj_fd = resolved.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_read() {
        return Err(FsBridgeError::PermDenied);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let data = BLOB_CACHE
            .get(&entry.blob_id)
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        if data.is_empty() {
            return Err(FsBridgeError::WouldBlock);
        }
        let read_len = count.min(data.len());
        copy_to_user(buf_ptr as *mut u8, data.as_ptr(), read_len)
            .map_err(|_| FsBridgeError::Fault)?;
        let remaining = data[read_len..].to_vec();
        BLOB_CACHE
            .insert(entry.blob_id, remaining)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&entry.blob_id);
        return Ok(read_len as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        let (value, flags) = eventfd_state(entry.blob_id)?;
        if value == 0 {
            return Err(FsBridgeError::WouldBlock);
        }
        let out = if flags & EFD_SEMAPHORE != 0 { 1 } else { value };
        let next = if flags & EFD_SEMAPHORE != 0 {
            value.saturating_sub(1)
        } else {
            0
        };
        write_user_typed(buf_ptr, out).map_err(|_| FsBridgeError::Fault)?;
        store_eventfd_state(entry.blob_id, next, flags)?;
        return Ok(size_of::<u64>() as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG) {
        return Err(FsBridgeError::WouldBlock);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let data = read_socket_payload(entry.blob_id, count, true)?;
        if data.is_empty() {
            return Err(FsBridgeError::WouldBlock);
        }
        copy_to_user(buf_ptr as *mut u8, data.as_ptr(), data.len())
            .map_err(|_| FsBridgeError::Fault)?;
        return Ok(data.len() as i64);
    }

    let start = entry.cursor as usize;
    let data = match BLOB_CACHE.read_at(&entry.blob_id, start, count) {
        Ok(data) => data,
        Err(ExofsError::BlobNotFound) if entry.size == 0 => return Ok(0),
        Err(err) => return Err(exofs_to_bridge_error(err)),
    };
    if data.is_empty() {
        return Ok(0);
    }
    let read_len = data.len();
    copy_to_user(buf_ptr as *mut u8, data.as_ptr(), read_len).map_err(|_| FsBridgeError::Fault)?;
    OBJECT_TABLE
        .advance_cursor(obj_fd, read_len as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(read_len as i64)
}

/// `write(fd, buf, count)` → octets écrits.
#[inline]
pub fn fs_write(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        if !fd_can_write_flags(resolved.flags) {
            return Err(FsBridgeError::BadFd);
        }
        return tty_write_from_user(pid, buf_ptr, count);
    }
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }

    let obj_fd = resolved.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        if OBJECT_TABLE.readable_count_for(&entry.blob_id) == 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
            return Err(FsBridgeError::BrokenPipe);
        }
        let input = read_user_bytes(buf_ptr, count)?;
        let mut data = BLOB_CACHE
            .get(&entry.blob_id)
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        data.try_reserve(input.len())
            .map_err(|_| FsBridgeError::NoSpace)?;
        data.extend_from_slice(&input);
        BLOB_CACHE
            .insert(entry.blob_id, data)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&entry.blob_id);
        return Ok(count as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        let add = read_user_typed::<u64>(buf_ptr).map_err(|_| FsBridgeError::Fault)?;
        if add == u64::MAX {
            return Err(FsBridgeError::Invalid);
        }
        let (value, flags) = eventfd_state(entry.blob_id)?;
        let next = value.checked_add(add).ok_or(FsBridgeError::WouldBlock)?;
        store_eventfd_state(entry.blob_id, next, flags)?;
        return Ok(size_of::<u64>() as i64);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let input = read_user_bytes(buf_ptr, count)?;
        let peer = socket_peer_blob(entry.blob_id)?;
        return append_socket_payload(peer, &input);
    }

    let input = read_user_bytes(buf_ptr, count)?;

    let start = if entry.flags & open_flags::O_APPEND != 0 {
        blob_len(&entry.blob_id)
    } else {
        entry.cursor as usize
    };
    let end = start.checked_add(count).ok_or(FsBridgeError::NoSpace)?;
    BLOB_CACHE
        .write_at(entry.blob_id, start, &input)
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_cursor(obj_fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_size(obj_fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    let _ = pid;
    Ok(count as i64)
}

/// `pread64(fd, buf, count, offset)` without moving the shared file cursor.
#[inline]
pub fn fs_pread64(
    fd: u32,
    buf_ptr: u64,
    count: usize,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_read() {
        return Err(FsBridgeError::PermDenied);
    }
    read_blob_at(entry.blob_id, offset, buf_ptr, count)
}

/// `pwrite64(fd, buf, count, offset)` without moving the shared file cursor.
#[inline]
pub fn fs_pwrite64(
    fd: u32,
    buf_ptr: u64,
    count: usize,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let written = write_blob_at(entry.blob_id, offset, buf_ptr, count)?;
    let new_size = offset.saturating_add(written as u64).max(entry.size);
    OBJECT_TABLE
        .set_size(obj_fd, new_size)
        .map_err(exofs_to_bridge_error)?;
    Ok(written)
}

/// `readv(fd, iov, iovcnt)`.
#[inline]
pub fn fs_readv(fd: u32, iov_ptr: u64, iovcnt: u32, pid: u32) -> Result<i64, FsBridgeError> {
    vectored_io(iov_ptr, iovcnt, |base, len| fs_read(fd, base, len, pid))
}

/// `writev(fd, iov, iovcnt)`.
#[inline]
pub fn fs_writev(fd: u32, iov_ptr: u64, iovcnt: u32, pid: u32) -> Result<i64, FsBridgeError> {
    vectored_io(iov_ptr, iovcnt, |base, len| fs_write(fd, base, len, pid))
}

/// `preadv(fd, iov, iovcnt, offset)`.
#[inline]
pub fn fs_preadv(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let mut pos = offset;
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let n = fs_pread64(fd, base, len, pos, pid)?;
        pos = pos.saturating_add(n as u64);
        Ok(n)
    })
}

/// `pwritev(fd, iov, iovcnt, offset)`.
#[inline]
pub fn fs_pwritev(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    offset: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let mut pos = offset;
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let n = fs_pwrite64(fd, base, len, pos, pid)?;
        pos = pos.saturating_add(n as u64);
        Ok(n)
    })
}

/// `lseek(fd, offset, whence)` → nouvelle position.
#[inline]
pub fn fs_lseek(fd: u32, offset: i64, whence: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    let base = match whence {
        SEEK_SET => 0i64,
        SEEK_CUR => entry.cursor as i64,
        SEEK_END => blob_len(&entry.blob_id) as i64,
        _ => return Err(FsBridgeError::Invalid),
    };
    let new_pos = base.checked_add(offset).ok_or(FsBridgeError::Invalid)?;
    if new_pos < 0 {
        return Err(FsBridgeError::Invalid);
    }
    OBJECT_TABLE
        .set_cursor(obj_fd, new_pos as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(new_pos)
}

/// `truncate(path, length)`.
#[inline]
pub fn fs_truncate(path: &[u8], length: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (blob_id, _) = path_entry(&normalized_path)?;
    resize_regular_blob(blob_id, length)?;
    Ok(0)
}

/// `ftruncate(fd, length)`.
#[inline]
pub fn fs_ftruncate(fd: u32, length: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    resize_regular_blob(entry.blob_id, length)?;
    OBJECT_TABLE
        .set_size(obj_fd, length)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `fsync(fd)` / `fdatasync(fd)`.
///
/// FIX-EXOFS-ROB-4 (AUDIT-EXOFS §3) : `fsync` durabilise désormais données ET
/// métadonnées (POSIX exige les deux). Pour `fdatasync` (`data_only == true`),
/// seules les données du blob sont forcées sur disque (pas de commit d'epoch des
/// métadonnées), conformément à la sémantique POSIX. L'ancien code persistait le
/// blob mais ignorait `data_only` et ne committait jamais d'epoch.
#[inline]
pub fn fs_fsync(fd: u32, data_only: bool, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;

    if !BLOB_CACHE.contains(&entry.blob_id) {
        return Ok(0);
    }

    // Forcer les données du blob sur le disque.
    if let Some(data) = BLOB_CACHE.get(&entry.blob_id) {
        match object_store::persist_blob_data_if_disk(entry.blob_id, &data, true) {
            Ok(_) => {
                let _ = BLOB_CACHE.mark_clean(&entry.blob_id);
            }
            Err(err) => return Err(exofs_to_bridge_error(err)),
        }
    }

    // fsync (≠ fdatasync) : sceller un epoch pour durabiliser les métadonnées
    // (PathIndex, taille, table d'objets) en plus des données. fdatasync s'arrête
    // après la persistance des données.
    if !data_only {
        match crate::fs::exofs::syscall::epoch_commit::commit_current_epoch() {
            Ok(_) | Err(ExofsError::CommitInProgress) => {}
            Err(err) => return Err(exofs_to_bridge_error(err)),
        }
    }

    Ok(0)
}

/// `copy_file_range`.
#[inline]
pub fn fs_copy_file_range(
    fd_in: u32,
    off_in_ptr: u64,
    fd_out: u32,
    off_out_ptr: u64,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_fd = resolve_fd(pid, fd_in)?.handle;
    let out_fd = resolve_fd(pid, fd_out)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    let in_offset = if off_in_ptr == 0 {
        in_entry.cursor
    } else {
        read_signed_offset(off_in_ptr)?
    };
    let out_offset = if off_out_ptr == 0 {
        out_entry.cursor
    } else {
        read_signed_offset(off_out_ptr)?
    };

    let data = read_blob_bytes_at(in_entry.blob_id, in_offset, len)?;
    if data.is_empty() {
        return Ok(0);
    }
    let written = write_blob_bytes_at(out_entry.blob_id, out_offset, &data)? as u64;
    if off_in_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(in_fd, in_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(off_in_ptr, (in_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    if off_out_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(out_fd, out_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(off_out_ptr, (out_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    OBJECT_TABLE
        .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
}

/// `sendfile(out_fd, in_fd, offset, count)`.
#[inline]
pub fn fs_sendfile(
    out_fd: u32,
    in_fd: u32,
    offset_ptr: u64,
    count: usize,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let in_fd = resolve_fd(pid, in_fd)?.handle;
    let out_fd = resolve_fd(pid, out_fd)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let in_offset = if offset_ptr == 0 {
        in_entry.cursor
    } else {
        read_signed_offset(offset_ptr)?
    };
    let out_offset = out_entry.cursor;
    let data = read_blob_bytes_at(in_entry.blob_id, in_offset, count)?;
    if data.is_empty() {
        return Ok(0);
    }
    let written = write_blob_bytes_at(out_entry.blob_id, out_offset, &data)? as u64;
    if offset_ptr == 0 {
        OBJECT_TABLE
            .set_cursor(in_fd, in_offset.saturating_add(written))
            .map_err(exofs_to_bridge_error)?;
    } else {
        write_user_typed(offset_ptr, (in_offset.saturating_add(written)) as i64)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    OBJECT_TABLE
        .set_cursor(out_fd, out_offset.saturating_add(written))
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(written as i64)
}

/// `fallocate(fd, mode, offset, len)`.
#[inline]
pub fn fs_fallocate(
    fd: u32,
    mode: u32,
    offset: u64,
    len: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if len == 0 || mode & !FALLOC_FL_KEEP_SIZE != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }
    let end = offset.checked_add(len).ok_or(FsBridgeError::Invalid)?;
    if mode & FALLOC_FL_KEEP_SIZE == 0 && end > blob_len(&entry.blob_id) as u64 {
        resize_regular_blob(entry.blob_id, end)?;
        OBJECT_TABLE
            .set_size(obj_fd, end)
            .map_err(exofs_to_bridge_error)?;
    } else {
        ensure_blob_exists(entry.blob_id)?;
    }
    Ok(0)
}

/// `posix_fadvise/fadvise64`.
#[inline]
pub fn fs_fadvise64(
    fd: u32,
    offset: u64,
    len: u64,
    advice: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (offset, len);
    if advice > 5 {
        return Err(FsBridgeError::Invalid);
    }
    let _ = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `ioctl(fd, request, arg)`.
#[inline]
pub fn fs_ioctl(fd: u32, request: u64, arg: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    if is_tty_handle(resolved.handle) {
        if request == 0x5402 || request == 0x5403 || request == 0x5404 || request == 0x5410 {
            let mut req = special::TtyRequestWire::zeroed();
            req.sender_pid = pid;
            req.msg_type = TTY_MSG_IOCTL;
            req.a = request;
            req.b = arg;
            let reply = special::tty_call(&req)?;
            return Ok(reply.status);
        }
    }
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    match request {
        FIONREAD => {
            if arg == 0 {
                return Err(FsBridgeError::Fault);
            }
            let available = if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
                eventfd_state(entry.blob_id)
                    .map(|(value, _)| if value == 0 { 0 } else { 8 })
                    .unwrap_or(0)
            } else if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
                blob_len(&entry.blob_id)
            } else if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
                socket_payload_len(entry.blob_id)
            } else {
                blob_len(&entry.blob_id).saturating_sub(entry.cursor as usize)
            };
            write_user_typed(arg, available as i32).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::Invalid),
    }
}

