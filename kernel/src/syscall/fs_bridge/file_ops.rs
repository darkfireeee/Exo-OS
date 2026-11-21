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

#[derive(Clone, Copy)]
pub struct SharedBlobPage {
    pub blob_id: BlobId,
    pub page_idx: u64,
    pub frame: crate::memory::core::Frame,
}

pub static SHARED_BLOB_FRAMES: Mutex<Vec<SharedBlobPage>> = Mutex::new(Vec::new());

pub fn get_or_alloc_shared_blob_frame(
    blob_id: BlobId,
    page_idx: u64,
) -> Result<crate::memory::core::Frame, crate::memory::virt::mmap::MmapError> {
    let mut table = SHARED_BLOB_FRAMES.lock();
    if let Some(entry) = table
        .iter()
        .find(|p| p.blob_id == blob_id && p.page_idx == page_idx)
    {
        return Ok(entry.frame);
    }

    if table.try_reserve(1).is_err() {
        return Err(crate::memory::virt::mmap::MmapError::AllocFailed);
    }

    let frame = crate::memory::physical::allocator::buddy::alloc_pages(
        0,
        crate::memory::AllocFlags::ZEROED,
    )
    .map_err(|_| crate::memory::virt::mmap::MmapError::AllocFailed)?;

    let offset = (page_idx as usize).saturating_mul(4096);
    if let Ok(data) = BLOB_CACHE.read_at(&blob_id, offset, 4096) {
        if !data.is_empty() {
            let dst = (crate::memory::core::PHYS_MAP_BASE.as_u64()
                + frame.start_address().as_u64()) as *mut u8;
            // SAFETY: `dst` points to a freshly allocated 4 KiB frame in the kernel physmap.
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len().min(4096));
            }
        }
    }

    table.push(SharedBlobPage {
        blob_id,
        page_idx,
        frame,
    });
    Ok(frame)
}

pub fn sync_shared_frames_to_blob(blob_id: &BlobId) {
    let pages: Vec<(u64, crate::memory::core::Frame)> = {
        let table = SHARED_BLOB_FRAMES.lock();
        table
            .iter()
            .filter(|p| p.blob_id == *blob_id)
            .map(|p| (p.page_idx, p.frame))
            .collect()
    };
    if pages.is_empty() {
        return;
    }
    let current_len = blob_len(blob_id);
    for (page_idx, frame) in pages {
        let start = (page_idx as usize).saturating_mul(4096);
        let write_len = if current_len > start {
            (current_len - start).min(4096)
        } else {
            4096
        };
        let src = (crate::memory::core::PHYS_MAP_BASE.as_u64()
            + frame.start_address().as_u64()) as *const u8;
        // SAFETY: `src` is a pinned 4 KiB physical frame mapped in the kernel physmap.
        let slice = unsafe { core::slice::from_raw_parts(src, write_len) };
        let _ = BLOB_CACHE.write_at(*blob_id, start, slice);
    }
}

pub fn sync_blob_write_to_shared_frames(blob_id: &BlobId, start: usize, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let end = start.saturating_add(bytes.len());
    let table = SHARED_BLOB_FRAMES.lock();
    for entry in table.iter().filter(|p| p.blob_id == *blob_id) {
        let page_start = (entry.page_idx as usize).saturating_mul(4096);
        let page_end = page_start.saturating_add(4096);
        let overlap_start = start.max(page_start);
        let overlap_end = end.min(page_end);
        if overlap_start < overlap_end {
            let src_off = overlap_start - start;
            let dst_off = overlap_start - page_start;
            let copy_len = overlap_end - overlap_start;
            let dst = (crate::memory::core::PHYS_MAP_BASE.as_u64()
                + entry.frame.start_address().as_u64()
                + dst_off as u64) as *mut u8;
            // SAFETY: `dst` + `copy_len` is within the 4 KiB frame in the kernel physmap.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes[src_off..].as_ptr(), dst, copy_len);
            }
        }
    }
}

/// Maps a file or `memfd` descriptor into the current process's virtual address space.
pub fn fs_mmap_file(
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
    fd: u32,
    off: u64,
    pid: u32,
) -> Result<usize, crate::memory::virt::mmap::MmapError> {
    use crate::memory::virt::mmap::{do_mmap, do_mmap_shared_frames, do_mprotect, do_munmap, MmapError};

    if prot & !0x7 != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if (prot & 0x2) != 0 && (prot & 0x4) != 0 {
        return Err(MmapError::PermissionDenied);
    }
    if (off & 0xFFF) != 0 {
        return Err(MmapError::InvalidAddress);
    }
    let resolved = resolve_fd(pid, fd).map_err(|_| MmapError::InvalidAddress)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(|_| MmapError::InvalidAddress)?;
    {
        use crate::fs::exofs::syscall::object_fd::open_flags;
        if !open_flags::can_read(entry.flags) {
            return Err(MmapError::PermissionDenied);
        }
        if (flags & 0x01) != 0 && (prot & 0x02) != 0 && !open_flags::can_write(entry.flags) {
            return Err(MmapError::PermissionDenied);
        }
    }

    let n_pages = (len + 4095) / 4096;
    if flags & 0x01 != 0 {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(n_pages)
            .map_err(|_| MmapError::AllocFailed)?;
        let base_page = off / 4096;
        for i in 0..n_pages {
            let frame = get_or_alloc_shared_blob_frame(entry.blob_id, base_page + i as u64)?;
            frames.push(frame);
        }
        return do_mmap_shared_frames(addr, len, prot, flags, &frames);
    }

    // MAP_PRIVATE file-backed mapping: map anonymous pages temporarily writable, populate from file,
    // then apply strict requested protections (`PROT_NONE` / `PROT_READ` / `PROT_READ|PROT_EXEC`).
    sync_shared_frames_to_blob(&entry.blob_id);
    let _ = ensure_blob_exists(entry.blob_id);
    let temp_prot = prot | 0x2; // Ensure writable during initial file copy
    let va = do_mmap(addr, len, temp_prot & 0x3, flags | 0x20, -1, 0)?;
    if let Ok(data) = BLOB_CACHE.read_at(&entry.blob_id, off as usize, len) {
        if !data.is_empty() {
            if copy_to_user(va as *mut u8, data.as_ptr(), data.len()).is_err() {
                let _ = do_munmap(va as u64, len);
                return Err(MmapError::InvalidAddress);
            }
        }
    }
    if prot != (temp_prot & 0x3) {
        if let Err(err) = do_mprotect(va as u64, len, prot) {
            let _ = do_munmap(va as u64, len);
            return Err(err);
        }
    }
    Ok(va)
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

    sync_shared_frames_to_blob(&blob_id);
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
    sync_blob_write_to_shared_frames(&blob_id, start, &input);
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
    sync_shared_frames_to_blob(&blob_id);
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
    sync_blob_write_to_shared_frames(&blob_id, start, bytes);
    Ok(bytes.len() as i64)
}

/// `read(fd, buf, count)` -> bytes read.
/// BRIDGE: `crate::fs::vfs::read(fd, buf_slice)` - enabled when `pub mod fs;`
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
    if blob_is_directory_by_id(&entry.blob_id) {
        return Err(FsBridgeError::IsDir);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        return read_pipe_to_user(entry.blob_id, buf_ptr, count, nonblock);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        return read_eventfd_to_user(entry.blob_id, buf_ptr);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG) {
        return Err(FsBridgeError::WouldBlock);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        return socket_read_to_user(entry.blob_id, buf_ptr, count, true, nonblock);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        let flags = if nonblock { 0x40 } else { 0 };
        return crate::syscall::net_bridge::net_recvfrom_blob(
            entry.blob_id,
            buf_ptr,
            count,
            flags,
            0,
            0,
        )
        .map_err(errno_to_fs_error);
    }

    sync_shared_frames_to_blob(&entry.blob_id);
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

/// `write(fd, buf, count)` -> bytes written.
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
    if blob_is_directory_by_id(&entry.blob_id) {
        return Err(FsBridgeError::IsDir);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        let input = read_user_bytes(buf_ptr, count)?;
        return write_pipe_bytes(entry.blob_id, &input, nonblock, pid);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG) {
        if count < size_of::<u64>() {
            return Err(FsBridgeError::Invalid);
        }
        return write_eventfd_from_user(entry.blob_id, buf_ptr);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        let input = read_user_bytes(buf_ptr, count)?;
        return write_unix_socket_bytes(entry.blob_id, &input, 0, nonblock, pid);
    }

    if is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG) {
        let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
        let flags = if nonblock { 0x40 } else { 0 };
        return crate::syscall::net_bridge::net_sendto_blob(
            entry.blob_id,
            buf_ptr,
            count,
            flags,
            0,
            0,
        )
        .map_err(errno_to_fs_error);
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
    sync_blob_write_to_shared_frames(&entry.blob_id, start, &input);
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
    if blob_is_directory_by_id(&entry.blob_id) {
        return Err(FsBridgeError::IsDir);
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
    if blob_is_directory_by_id(&entry.blob_id) {
        return Err(FsBridgeError::IsDir);
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

/// `lseek(fd, offset, whence)` -> new position.
#[inline]
pub fn fs_lseek(fd: u32, offset: i64, whence: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let obj_fd = resolve_fd(pid, fd)?.handle;
    let entry = OBJECT_TABLE.get(obj_fd).map_err(exofs_to_bridge_error)?;
    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG)
    {
        return Err(FsBridgeError::IllegalSeek);
    }
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
    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_EVENTFD_TAG)
        || is_pseudo_blob(&entry.blob_id, PSEUDO_INOTIFY_TAG)
    {
        return Err(FsBridgeError::Invalid);
    }
    resize_regular_blob(entry.blob_id, length)?;
    OBJECT_TABLE
        .set_size(obj_fd, length)
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `fsync(fd)` / `fdatasync(fd)`.
///
/// FIX-EXOFS-ROB-4 (AUDIT-EXOFS §3): `fsync` now persists both data AND
/// metadata (POSIX requires both). For `fdatasync` (`data_only == true`),
/// only blob data is flushed to disk (no metadata epoch commit),
/// in accordance with POSIX semantics. The former code persisted the
/// blob but ignored `data_only` and never committed an epoch.
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

    // Flush blob data to disk.
    if let Some(data) = BLOB_CACHE.get(&entry.blob_id) {
        match object_store::persist_blob_data_if_disk(entry.blob_id, &data, true) {
            Ok(_) => {
                let _ = BLOB_CACHE.mark_clean(&entry.blob_id);
            }
            Err(err) => return Err(exofs_to_bridge_error(err)),
        }
    }

    // fsync (!= fdatasync): seal an epoch to persist metadata
    // (PathIndex, size, object table) in addition to data. fdatasync stops
    // after data persistence.
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

    // FIONBIO (0x5421): toggle O_NONBLOCK on any descriptor (files, pipes, sockets, TTY).
    if request == FIONBIO {
        if arg == 0 {
            return Err(FsBridgeError::Fault);
        }
        let nonblock_flag = read_user_typed::<i32>(arg).map_err(|_| FsBridgeError::Fault)?;
        let cur_flags = fs_fcntl(fd, F_GETFL, 0, pid)? as u32;
        let next_flags = if nonblock_flag != 0 {
            cur_flags | O_NONBLOCK
        } else {
            cur_flags & !O_NONBLOCK
        };
        let _ = fs_fcntl(fd, F_SETFL, next_flags as u64, pid)?;
        return Ok(0);
    }

    if is_tty_handle(resolved.handle) {
        if request == 0x5413 {
            // TIOCGWINSZ: struct winsize { u16 ws_row, ws_col, ws_xpixel, ws_ypixel }
            if arg == 0 {
                return Err(FsBridgeError::Fault);
            }
            let (px_w, px_h) = crate::arch::x86_64::framebuffer_early::user_framebuffer_info()
                .map(|(_, w, h, _, _, _, _)| (w, h))
                .unwrap_or((1920, 1080));
            let rows = (px_h.saturating_sub(240) / 16).clamp(24, u16::MAX as u32) as u16;
            let cols = (px_w.saturating_sub(520) / 8).clamp(80, u16::MAX as u32) as u16;
            let ws: [u16; 4] = [
                rows,
                cols,
                px_w.min(u16::MAX as u32) as u16,
                px_h.min(u16::MAX as u32) as u16,
            ];
            write_user_typed(arg, ws).map_err(|_| FsBridgeError::Fault)?;
            return Ok(0);
        }
        if request == 0x5401 {
            // TCGETS: query real line discipline state from tty_server and populate Linux termios (60 bytes).
            if arg == 0 {
                return Err(FsBridgeError::Fault);
            }
            let mut req = special::TtyRequestWire::zeroed();
            req.sender_pid = pid;
            req.msg_type = TTY_MSG_IOCTL;
            req.a = request;
            req.b = 0;
            let reply = special::tty_call(&req)?;
            if reply.status < 0 {
                return Err(errno_to_fs_error(reply.status));
            }
            let mode_bits = reply.signal; // bit 0 = echo, bit 1 = canonical
            let mut termios = [0u8; 60];
            let c_iflag: u32 = 0x0100; // ICRNL
            let c_oflag: u32 = 0x0001 | 0x0004; // OPOST | ONLCR
            let c_cflag: u32 = 0x0030 | 0x0080 | 0x000f; // CS8 | CREAD | B38400
            let mut c_lflag: u32 = 0x0001; // ISIG
            if (mode_bits & 1) != 0 {
                c_lflag |= 0x0008; // ECHO
            }
            if (mode_bits & 2) != 0 {
                c_lflag |= 0x0002; // ICANON
            }
            termios[0..4].copy_from_slice(&c_iflag.to_ne_bytes());
            termios[4..8].copy_from_slice(&c_oflag.to_ne_bytes());
            termios[8..12].copy_from_slice(&c_cflag.to_ne_bytes());
            termios[12..16].copy_from_slice(&c_lflag.to_ne_bytes());
            // c_cc starts at offset 17 in Linux x86_64 termios
            termios[17 + 0] = 3; // VINTR = ^C
            termios[17 + 1] = 28; // VQUIT = ^\
            termios[17 + 2] = 127; // VERASE = DEL
            termios[17 + 3] = 21; // VKILL = ^U
            termios[17 + 4] = 4; // VEOF = ^D
            termios[17 + 6] = 1; // VMIN = 1
            copy_to_user(arg as *mut u8, termios.as_ptr(), termios.len())
                .map_err(|_| FsBridgeError::Fault)?;
            return Ok(0);
        }
        if request == 0x5402 || request == 0x5403 || request == 0x5404 {
            // TCSETS/TCSETSW/TCSETSF: accept either Linux termios* (>= 0x1000) or exosh raw mode bits (<= 3).
            let mode_bits = if arg <= 3 {
                arg
            } else {
                let mut termios = [0u8; 60];
                copy_from_user(termios.as_mut_ptr(), arg as *const u8, termios.len())
                    .map_err(|_| FsBridgeError::Fault)?;
                let c_lflag = u32::from_ne_bytes([
                    termios[12],
                    termios[13],
                    termios[14],
                    termios[15],
                ]);
                let echo = if (c_lflag & 0x0008) != 0 { 1u64 } else { 0 };
                let canon = if (c_lflag & 0x0002) != 0 { 2u64 } else { 0 };
                echo | canon
            };
            let mut req = special::TtyRequestWire::zeroed();
            req.sender_pid = pid;
            req.msg_type = TTY_MSG_IOCTL;
            req.a = request;
            req.b = mode_bits;
            let reply = special::tty_call(&req)?;
            if reply.status < 0 {
                return Err(errno_to_fs_error(reply.status));
            }
            return Ok(reply.status);
        }
        if request == 0x5410 {
            let mut req = special::TtyRequestWire::zeroed();
            req.sender_pid = pid;
            req.msg_type = TTY_MSG_IOCTL;
            req.a = request;
            req.b = arg;
            let reply = special::tty_call(&req)?;
            if reply.status < 0 {
                return Err(errno_to_fs_error(reply.status));
            }
            return Ok(reply.status);
        }
        return Err(FsBridgeError::NotTty);
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
                if socket_is_dgram(entry.blob_id) {
                    dgram_next_packet_len(entry.blob_id)
                } else {
                    socket_payload_len(entry.blob_id)
                }
            } else if is_pseudo_blob(&entry.blob_id, PSEUDO_NET_SOCKET_TAG) {
                crate::syscall::net_bridge::net_available_bytes_for_blob(entry.blob_id)
            } else {
                blob_len(&entry.blob_id).saturating_sub(entry.cursor as usize)
            };
            write_user_typed(arg, available as i32).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::NotTty),
    }
}

