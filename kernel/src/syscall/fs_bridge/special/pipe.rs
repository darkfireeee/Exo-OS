//! # syscall/fs_bridge/special/pipe.rs — Anonymous POSIX pipes (`pipe2`, bounded FIFO)

use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{copy_to_user, write_user_typed};

static PIPE_PAYLOAD_LOCK: Mutex<()> = Mutex::new(());

pub const PIPE_MAX_CAPACITY: usize = 65536;

#[inline]
pub fn read_pipe_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let out = {
        let _guard = PIPE_PAYLOAD_LOCK.lock();
        let data = BLOB_CACHE
            .get(&blob_id)
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let read_len = count.min(data.len());
        let out = data[..read_len].to_vec();
        if consume {
            BLOB_CACHE
                .insert(blob_id, data[read_len..].to_vec())
                .map_err(exofs_to_bridge_error)?;
            let _ = BLOB_CACHE.mark_dirty(&blob_id);
        }
        out
    };
    if consume && !out.is_empty() {
        notify_io_event(Some(blob_id));
    }
    Ok(out)
}

/// Read up to `count` bytes from pipe `blob_id` into user `buf_ptr`, consuming only after copy succeeds.
pub fn read_pipe_to_user(
    blob_id: BlobId,
    buf_ptr: u64,
    count: usize,
    nonblock: bool,
) -> Result<i64, FsBridgeError> {
    if count == 0 {
        return Ok(0);
    }
    loop {
        let gen = io_wait_gen();
        {
            let _guard = PIPE_PAYLOAD_LOCK.lock();
            let data = BLOB_CACHE
                .get(&blob_id)
                .map(|bytes| bytes.to_vec())
                .unwrap_or_default();
            if !data.is_empty() {
                let read_len = count.min(data.len());
                copy_to_user(buf_ptr as *mut u8, data.as_ptr(), read_len)
                    .map_err(|_| FsBridgeError::Fault)?;
                let remaining = data[read_len..].to_vec();
                BLOB_CACHE
                    .insert(blob_id, remaining)
                    .map_err(exofs_to_bridge_error)?;
                let _ = BLOB_CACHE.mark_dirty(&blob_id);
                drop(_guard);
                notify_io_event(Some(blob_id));
                return Ok(read_len as i64);
            }
        }
        if OBJECT_TABLE.open_count_for(&blob_id) <= OBJECT_TABLE.readable_count_for(&blob_id) {
            return Ok(0);
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
    }
}

#[inline]
fn append_pipe_payload_locked(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = BLOB_CACHE
        .get(&blob_id)
        .map(|existing| existing.to_vec())
        .unwrap_or_default();
    if data.len() >= PIPE_MAX_CAPACITY {
        return Err(FsBridgeError::WouldBlock);
    }
    let to_write = bytes.len().min(PIPE_MAX_CAPACITY - data.len());
    data.try_reserve(to_write)
        .map_err(|_| FsBridgeError::NoSpace)?;
    data.extend_from_slice(&bytes[..to_write]);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(to_write as i64)
}

#[inline]
pub fn append_pipe_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    let written = {
        let _guard = PIPE_PAYLOAD_LOCK.lock();
        append_pipe_payload_locked(blob_id, bytes)?
    };
    if written > 0 {
        notify_io_event(Some(blob_id));
    }
    Ok(written)
}

/// Write `input` bytes to pipe `blob_id`, checking `SIGPIPE` and `PIPE_MAX_CAPACITY`.
pub fn write_pipe_bytes(
    blob_id: BlobId,
    input: &[u8],
    nonblock: bool,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    loop {
        let gen = io_wait_gen();
        if OBJECT_TABLE.readable_count_for(&blob_id) == 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
            return Err(FsBridgeError::BrokenPipe);
        }
        if input.is_empty() {
            return Ok(0);
        }
        match append_pipe_payload(blob_id, input) {
            Ok(n) => return Ok(n),
            Err(FsBridgeError::WouldBlock) if !nonblock => {
                let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
            }
            Err(e) => return Err(e),
        }
    }
}

/// `pipe2(pipefd, flags)`.
#[inline]
pub fn fs_pipe2(fds_ptr: u64, flags: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if fds_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(FsBridgeError::Invalid);
    }

    let blob_id = next_pseudo_blob(PSEUDO_PIPE_TAG);
    ensure_blob_exists(blob_id)?;
    let read_fd = OBJECT_TABLE
        .open(blob_id, open_flags::O_RDONLY, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    let write_fd = match OBJECT_TABLE.open(
        blob_id,
        open_flags::O_WRONLY | open_flags::O_APPEND,
        0,
        0,
        pid as u64,
    ) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = OBJECT_TABLE.close(read_fd);
            return Err(exofs_to_bridge_error(err));
        }
    };

    let mut fds = [read_fd as i32, write_fd as i32];
    let has_fd_table = process_has_fd_table(pid);
    if has_fd_table {
        let read_flags = fd_table_flags(flags, open_flags::O_RDONLY);
        let write_flags = fd_table_flags(flags, open_flags::O_WRONLY | open_flags::O_APPEND);
        let Some(read_logical) = install_process_fd(pid, read_fd as u64, read_flags) else {
            let _ = OBJECT_TABLE.close(read_fd);
            let _ = OBJECT_TABLE.close(write_fd);
            return Err(FsBridgeError::NoMemory);
        };
        let Some(write_logical) = install_process_fd(pid, write_fd as u64, write_flags) else {
            let _ = close_process_fd(pid, read_logical as u32);
            let _ = OBJECT_TABLE.close(read_fd);
            let _ = OBJECT_TABLE.close(write_fd);
            return Err(FsBridgeError::NoMemory);
        };
        fds = [read_logical, write_logical];
    }
    if write_user_typed(fds_ptr, fds).is_err() {
        if has_fd_table {
            let _ = close_process_fd(pid, fds[0] as u32);
            let _ = close_process_fd(pid, fds[1] as u32);
        }
        let _ = OBJECT_TABLE.close(read_fd);
        let _ = OBJECT_TABLE.close(write_fd);
        return Err(FsBridgeError::Fault);
    }
    Ok(0)
}
