//! # syscall/fs_bridge/special/eventfd.rs — POSIX/Linux `eventfd2` pseudo-descriptor

use alloc::vec::Vec;
use core::mem::size_of;
use spin::Mutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{read_user_typed, write_user_typed};

static EVENTFD_LOCK: Mutex<()> = Mutex::new(());

#[inline]
pub fn eventfd_state(blob_id: BlobId) -> Result<(u64, u32), FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() < 8 {
        return Err(FsBridgeError::Invalid);
    }
    let value = u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let flags = if data.len() > 8 { data[8] as u32 } else { 0 };
    Ok((value, flags))
}

#[inline]
pub fn store_eventfd_state(blob_id: BlobId, value: u64, flags: u32) -> Result<(), FsBridgeError> {
    let mut data = Vec::new();
    data.extend_from_slice(&value.to_le_bytes());
    data.push((flags & EFD_SEMAPHORE) as u8);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(())
}

#[inline]
pub fn read_eventfd_to_user(blob_id: BlobId, buf_ptr: u64) -> Result<i64, FsBridgeError> {
    let _guard = EVENTFD_LOCK.lock();
    let (value, flags) = eventfd_state(blob_id)?;
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
    store_eventfd_state(blob_id, next, flags)?;
    drop(_guard);
    notify_io_event(Some(blob_id));
    Ok(size_of::<u64>() as i64)
}

#[inline]
pub fn write_eventfd_from_user(blob_id: BlobId, buf_ptr: u64) -> Result<i64, FsBridgeError> {
    let add = read_user_typed::<u64>(buf_ptr).map_err(|_| FsBridgeError::Fault)?;
    if add == u64::MAX {
        return Err(FsBridgeError::Invalid);
    }
    let _guard = EVENTFD_LOCK.lock();
    let (value, flags) = eventfd_state(blob_id)?;
    let next = value.checked_add(add).ok_or(FsBridgeError::WouldBlock)?;
    store_eventfd_state(blob_id, next, flags)?;
    drop(_guard);
    notify_io_event(Some(blob_id));
    Ok(size_of::<u64>() as i64)
}

/// `eventfd2(initval, flags)`.
#[inline]
pub fn fs_eventfd2(initval: u32, flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK | EFD_SEMAPHORE) != 0 {
        return Err(FsBridgeError::Invalid);
    }

    let blob_id = next_pseudo_blob(PSEUDO_EVENTFD_TAG);
    store_eventfd_state(blob_id, initval as u64, flags)?;
    let fd = OBJECT_TABLE
        .open(
            blob_id,
            open_flags::O_RDWR,
            size_of::<u64>() as u64,
            0,
            pid as u64,
        )
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        if let Some(logical_fd) =
            install_process_fd(pid, fd as u64, fd_table_flags(flags, open_flags::O_RDWR))
        {
            Ok(logical_fd as i64)
        } else {
            let _ = OBJECT_TABLE.close(fd);
            Err(FsBridgeError::NoMemory)
        }
    } else {
        Ok(fd as i64)
    }
}
