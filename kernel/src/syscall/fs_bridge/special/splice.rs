//! # syscall/fs_bridge/special/splice.rs — `splice`, `tee` & `vmsplice` operations

use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::write_user_typed;

/// `splice(fd_in, off_in, fd_out, off_out, len, flags)`.
#[inline]
pub fn fs_splice(
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
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_fd = resolve_fd(pid, fd_in)?.handle;
    let out_fd = resolve_fd(pid, fd_out)?.handle;
    let in_entry = OBJECT_TABLE.get(in_fd).map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE.get(out_fd).map_err(exofs_to_bridge_error)?;
    if !in_entry.can_read() || !out_entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    let input_is_pipe = is_pseudo_blob(&in_entry.blob_id, PSEUDO_PIPE_TAG);
    let output_is_pipe = is_pseudo_blob(&out_entry.blob_id, PSEUDO_PIPE_TAG);
    let input_is_socket = is_pseudo_blob(&in_entry.blob_id, PSEUDO_SOCKET_TAG);
    let output_is_socket = is_pseudo_blob(&out_entry.blob_id, PSEUDO_SOCKET_TAG);

    let data = if input_is_pipe {
        read_pipe_payload(in_entry.blob_id, len, true)?
    } else if input_is_socket {
        read_socket_payload(in_entry.blob_id, len, true)?
    } else {
        let offset = if off_in_ptr == 0 {
            in_entry.cursor
        } else {
            read_signed_offset(off_in_ptr)?
        };
        let data = read_blob_bytes_at(in_entry.blob_id, offset, len)?;
        if off_in_ptr == 0 {
            OBJECT_TABLE
                .set_cursor(in_fd, offset.saturating_add(data.len() as u64))
                .map_err(exofs_to_bridge_error)?;
        } else {
            write_user_typed(
                off_in_ptr,
                (offset.saturating_add(data.len() as u64)) as i64,
            )
            .map_err(|_| FsBridgeError::Fault)?;
        }
        data
    };
    if data.is_empty() {
        return Ok(0);
    }

    if output_is_pipe {
        append_pipe_payload(out_entry.blob_id, &data)
    } else if output_is_socket {
        let peer = socket_peer_blob(out_entry.blob_id)?;
        append_socket_payload(peer, &data)
    } else {
        let offset = if off_out_ptr == 0 {
            out_entry.cursor
        } else {
            read_signed_offset(off_out_ptr)?
        };
        let written = write_blob_bytes_at(out_entry.blob_id, offset, &data)? as u64;
        if off_out_ptr == 0 {
            OBJECT_TABLE
                .set_cursor(out_fd, offset.saturating_add(written))
                .map_err(exofs_to_bridge_error)?;
        } else {
            write_user_typed(off_out_ptr, (offset.saturating_add(written)) as i64)
                .map_err(|_| FsBridgeError::Fault)?;
        }
        OBJECT_TABLE
            .set_size(out_fd, blob_len(&out_entry.blob_id) as u64)
            .map_err(exofs_to_bridge_error)?;
        Ok(written as i64)
    }
}

/// `tee(fd_in, fd_out, len, flags)` duplicates pipe data without consuming it.
#[inline]
pub fn fs_tee(
    fd_in: u32,
    fd_out: u32,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let in_entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd_in)?.handle)
        .map_err(exofs_to_bridge_error)?;
    let out_entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd_out)?.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&in_entry.blob_id, PSEUDO_PIPE_TAG)
        || !is_pseudo_blob(&out_entry.blob_id, PSEUDO_PIPE_TAG)
    {
        return Err(FsBridgeError::Invalid);
    }
    let data = read_pipe_payload(in_entry.blob_id, len, false)?;
    append_pipe_payload(out_entry.blob_id, &data)
}

/// `vmsplice(fd, iov, nr_segs, flags)` writes user iovecs into a pipe.
#[inline]
pub fn fs_vmsplice(
    fd: u32,
    iov_ptr: u64,
    iovcnt: u32,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if flags & !0x0F != 0 {
        return Err(FsBridgeError::Invalid);
    }
    let entry = OBJECT_TABLE
        .get(resolve_fd(pid, fd)?.handle)
        .map_err(exofs_to_bridge_error)?;
    if !entry.can_write() || !is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    vectored_io(iov_ptr, iovcnt, |base, len| {
        let input = read_user_bytes(base, len)?;
        append_pipe_payload(entry.blob_id, &input)
    })
}
