//! # syscall/fs_bridge/special.rs

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

/// FIX-V8 : journalise une seule fois le mode console E9 avant tty_server.
#[cfg(target_arch = "x86_64")]
pub static TTY_E9ONLY_LOGGED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TtyRequestWire {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub a: u64,
    pub b: u64,
    pub data: [u8; TTY_LINE_MAX],
}

impl TtyRequestWire {
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            reply_endpoint: 0,
            a: 0,
            b: 0,
            data: [0; TTY_LINE_MAX],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TtyReplyWire {
    pub status: i64,
    pub signal: u32,
    pub len: u32,
    pub data: [u8; TTY_LINE_MAX],
}

pub struct TtyInputBuffer {
    pub data: [u8; TTY_LINE_MAX],
    pub head: usize,
    pub len: usize,
}

impl TtyInputBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; TTY_LINE_MAX],
            head: 0,
            len: 0,
        }
    }

    pub fn push_slice(&mut self, bytes: &[u8]) {
        self.head = 0;
        self.len = bytes.len().min(TTY_LINE_MAX);
        self.data[..self.len].copy_from_slice(&bytes[..self.len]);
    }

    pub fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        if n == 0 {
            return 0;
        }
        out[..n].copy_from_slice(&self.data[self.head..self.head + n]);
        self.head += n;
        self.len -= n;
        if self.len == 0 {
            self.head = 0;
        }
        n
    }
}

#[inline]
pub fn is_tty_handle(handle: u32) -> bool {
    handle == TTY_PTS0_HANDLE
}

#[inline]
pub fn is_tty_handle_u64(handle: u64) -> bool {
    handle == TTY_PTS0_HANDLE as u64
}

#[inline]
pub fn is_tty_path(path: &[u8]) -> bool {
    path == b"/dev/pts/0" || path == b"/dev/tty"
}

pub fn tty_endpoint() -> Result<EndpointId, FsBridgeError> {
    let cached = TTY_ENDPOINT_CACHE.load(Ordering::Acquire);
    if cached != 0 {
        if let Some(endpoint) = EndpointId::new(cached) {
            return Ok(endpoint);
        }
        TTY_ENDPOINT_CACHE.store(0, Ordering::Release);
    }

    let endpoint = crate::ipc::endpoint::lookup_endpoint(TTY_SERVER_ENDPOINT_NAME)
        .ok_or(FsBridgeError::NotReady)?;
    TTY_ENDPOINT_CACHE.store(endpoint.get(), Ordering::Release);
    Ok(endpoint)
}

pub fn tty_call(req: &TtyRequestWire) -> Result<TtyReplyWire, FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let mut reply_buf = [0u8; size_of::<TtyReplyWire>()];
    match crate::ipc::rpc::call_raw(endpoint, request, &mut reply_buf) {
        Ok(n) if n >= size_of::<TtyReplyWire>() => {
            Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const TtyReplyWire) })
        }
        Ok(_) => Err(FsBridgeError::Invalid),
        Err(err) => Err(ipc_to_fs_error(err)),
    }
}

pub fn tty_send(req: &TtyRequestWire) -> Result<(), FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(TTY_SEND_TIMEOUT_NS);

    loop {
        match crate::ipc::channel::raw::try_send_raw_nowait(endpoint, request) {
            Ok(_) => return Ok(()),
            Err(IpcError::WouldBlock) | Err(IpcError::QueueFull) | Err(IpcError::Full) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    return Err(FsBridgeError::WouldBlock);
                }
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            Err(err) => return Err(ipc_to_fs_error(err)),
        }
    }
}

pub fn tty_write_bytes(pid: u32, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    // Tap console headless : recopie la sortie console (fd 1) sur le port debug E9.
    // Permet d'observer init/serveurs/shell sous QEMU `-display none -debugcon`.
    crate::arch::x86_64::terminal::debug_write(bytes);
    let mut written = 0usize;
    while written < bytes.len() {
        let n = (bytes.len() - written).min(TTY_LINE_MAX);
        let mut req = TtyRequestWire::zeroed();
        req.sender_pid = pid;
        req.msg_type = TTY_MSG_WRITE;
        req.a = n as u64;
        req.data[..n].copy_from_slice(&bytes[written..written + n]);
        match tty_send(&req) {
            Ok(()) => {}
            Err(FsBridgeError::NotReady) => {
                #[cfg(target_arch = "x86_64")]
                if !TTY_E9ONLY_LOGGED.swap(true, Ordering::AcqRel) {
                    crate::arch::x86_64::terminal::debug_write(b"<TTY-E9ONLY>\n");
                }
                return Ok(bytes.len() as i64);
            }
            Err(e) => return Err(e),
        }
        written += n;
    }
    Ok(bytes.len() as i64)
}

pub fn tty_refill_stdin(pid: u32) -> Result<(), FsBridgeError> {
    let mut req = TtyRequestWire::zeroed();
    req.sender_pid = pid;
    req.msg_type = TTY_MSG_READ_LINE;
    let reply = tty_call(&req)?;
    if reply.status < 0 {
        return Err(errno_to_fs_error(reply.status));
    }
    let n = (reply.len as usize).min(TTY_LINE_MAX);
    if n == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    TTY_STDIN.lock().push_slice(&reply.data[..n]);
    Ok(())
}

pub fn tty_read_bytes(buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    let mut out = [0u8; TTY_LINE_MAX];
    let mut copied = TTY_STDIN
        .lock()
        .pop_into(&mut out[..count.min(TTY_LINE_MAX)]);
    if copied == 0 {
        tty_refill_stdin(pid)?;
        copied = TTY_STDIN
            .lock()
            .pop_into(&mut out[..count.min(TTY_LINE_MAX)]);
    }
    if copied == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    copy_to_user(buf_ptr as *mut u8, out.as_ptr(), copied).map_err(|_| FsBridgeError::Fault)?;
    Ok(copied as i64)
}

#[inline]
pub fn pseudo_blob_id(tag: u8, seq: u64) -> BlobId {
    let mut bytes = [0u8; 32];
    bytes[0] = tag;
    bytes[1] = b'E';
    bytes[2] = b'X';
    bytes[3] = b'O';
    bytes[4..12].copy_from_slice(&seq.to_le_bytes());
    BlobId(bytes)
}

#[inline]
pub fn next_pseudo_blob(tag: u8) -> BlobId {
    let seq = NEXT_PSEUDO_ID.fetch_add(1, Ordering::Relaxed);
    pseudo_blob_id(tag, seq)
}

#[inline]
pub fn is_pseudo_blob(blob_id: &BlobId, tag: u8) -> bool {
    let bytes = blob_id.as_bytes();
    bytes[0] == tag && bytes[1] == b'E' && bytes[2] == b'X' && bytes[3] == b'O'
}

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
pub fn socket_blob_with_peer(peer: BlobId) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(peer.as_bytes());
    data
}

#[inline]
pub fn socket_peer_blob(blob_id: BlobId) -> Result<BlobId, FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&data[..SOCKET_HEADER_LEN]);
    Ok(BlobId(bytes))
}

#[inline]
pub fn socket_payload_len(blob_id: BlobId) -> usize {
    BLOB_CACHE
        .get(&blob_id)
        .map(|data| data.len().saturating_sub(SOCKET_HEADER_LEN))
        .unwrap_or(0)
}

#[inline]
pub fn read_socket_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let data = snapshot_blob(&blob_id)?;
    if data.len() <= SOCKET_HEADER_LEN {
        return Ok(Vec::new());
    }
    let read_len = count.min(data.len() - SOCKET_HEADER_LEN);
    let out = data[SOCKET_HEADER_LEN..SOCKET_HEADER_LEN + read_len].to_vec();
    if consume {
        let mut next = data[..SOCKET_HEADER_LEN].to_vec();
        next.extend_from_slice(&data[SOCKET_HEADER_LEN + read_len..]);
        BLOB_CACHE
            .insert(blob_id, next)
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }
    Ok(out)
}

#[inline]
pub fn append_socket_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = snapshot_blob(&blob_id)?;
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    data.try_reserve(bytes.len())
        .map_err(|_| FsBridgeError::NoSpace)?;
    data.extend_from_slice(bytes);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(bytes.len() as i64)
}

#[inline]
pub fn read_pipe_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
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
    Ok(out)
}

#[inline]
pub fn append_pipe_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = BLOB_CACHE
        .get(&blob_id)
        .map(|existing| existing.to_vec())
        .unwrap_or_default();
    data.try_reserve(bytes.len())
        .map_err(|_| FsBridgeError::NoSpace)?;
    data.extend_from_slice(bytes);
    BLOB_CACHE
        .insert(blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(bytes.len() as i64)
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

/// `socketpair(AF_UNIX, SOCK_STREAM|SOCK_DGRAM, 0, sv)`.
#[inline]
pub fn fs_socketpair(
    domain: i32,
    ty: i32,
    protocol: i32,
    sv_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    pub const AF_UNIX: i32 = 1;
    pub const SOCK_STREAM: i32 = 1;
    pub const SOCK_DGRAM: i32 = 2;
    let clean_ty = ty & !((O_NONBLOCK | O_CLOEXEC) as i32);
    if domain != AF_UNIX || protocol != 0 || (clean_ty != SOCK_STREAM && clean_ty != SOCK_DGRAM) {
        return Err(FsBridgeError::Invalid);
    }
    if sv_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }

    let a_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    let b_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    BLOB_CACHE
        .insert(a_blob, socket_blob_with_peer(b_blob))
        .map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(b_blob, socket_blob_with_peer(a_blob))
        .map_err(exofs_to_bridge_error)?;

    let a_fd = OBJECT_TABLE
        .open(a_blob, open_flags::O_RDWR, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    let b_fd = match OBJECT_TABLE.open(b_blob, open_flags::O_RDWR, 0, 0, pid as u64) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = OBJECT_TABLE.close(a_fd);
            return Err(exofs_to_bridge_error(err));
        }
    };
    let mut sv = [a_fd as i32, b_fd as i32];
    let has_fd_table = process_has_fd_table(pid);
    if has_fd_table {
        let fd_flags = fd_table_flags(ty as u32, open_flags::O_RDWR);
        let Some(a_logical) = install_process_fd(pid, a_fd as u64, fd_flags) else {
            let _ = OBJECT_TABLE.close(a_fd);
            let _ = OBJECT_TABLE.close(b_fd);
            return Err(FsBridgeError::NoMemory);
        };
        let Some(b_logical) = install_process_fd(pid, b_fd as u64, fd_flags) else {
            let _ = close_process_fd(pid, a_logical as u32);
            let _ = OBJECT_TABLE.close(a_fd);
            let _ = OBJECT_TABLE.close(b_fd);
            return Err(FsBridgeError::NoMemory);
        };
        sv = [a_logical, b_logical];
    }
    if write_user_typed(sv_ptr, sv).is_err() {
        if has_fd_table {
            let _ = close_process_fd(pid, sv[0] as u32);
            let _ = close_process_fd(pid, sv[1] as u32);
        }
        let _ = OBJECT_TABLE.close(a_fd);
        let _ = OBJECT_TABLE.close(b_fd);
        return Err(FsBridgeError::Fault);
    }
    Ok(0)
}

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

