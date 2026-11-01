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
    // Tap console headless : recopie la sortie console sur le port debug E9 uniquement en trace debug.
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
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
                {
                    if !TTY_E9ONLY_LOGGED.swap(true, Ordering::AcqRel) {
                        crate::arch::x86_64::terminal::debug_write(b"<TTY-E9ONLY>\n");
                    }
                    crate::arch::x86_64::terminal::debug_write(&bytes[written..]);
                }
                return Ok(bytes.len() as i64);
            }
            Err(e) => return Err(e),
        }
        written += n;
    }
    Ok(bytes.len() as i64)
}

/// Écriture TTY sans allocation sur le heap : copie les tranches de mémoire utilisateur
/// directement dans les requêtes de ligne TTY allouées sur la pile (184 octets max).
pub fn tty_write_from_user(pid: u32, buf_ptr: u64, count: usize) -> Result<i64, FsBridgeError> {
    let mut written = 0usize;
    while written < count {
        let n = (count - written).min(TTY_LINE_MAX);
        let mut req = TtyRequestWire::zeroed();
        req.sender_pid = pid;
        req.msg_type = TTY_MSG_WRITE;
        req.a = n as u64;
        let uptr = (buf_ptr as usize).checked_add(written).ok_or(FsBridgeError::Fault)? as *const u8;
        copy_from_user(req.data.as_mut_ptr(), uptr, n).map_err(|_| FsBridgeError::Fault)?;
        #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
        crate::arch::x86_64::terminal::debug_write(&req.data[..n]);

        match tty_send(&req) {
            Ok(()) => {}
            Err(FsBridgeError::NotReady) => {
                #[cfg(target_arch = "x86_64")]
                {
                    if !TTY_E9ONLY_LOGGED.swap(true, Ordering::AcqRel) {
                        crate::arch::x86_64::terminal::debug_write(b"<TTY-E9ONLY>\n");
                    }
                    crate::arch::x86_64::terminal::debug_write(&req.data[..n]);
                    let mut rem = count - written - n;
                    let mut cur = written + n;
                    while rem > 0 {
                        let c = rem.min(TTY_LINE_MAX);
                        let ptr = (buf_ptr as usize).checked_add(cur).ok_or(FsBridgeError::Fault)? as *const u8;
                        let mut tmp = [0u8; TTY_LINE_MAX];
                        copy_from_user(tmp.as_mut_ptr(), ptr, c).map_err(|_| FsBridgeError::Fault)?;
                        crate::arch::x86_64::terminal::debug_write(&tmp[..c]);
                        cur += c;
                        rem -= c;
                    }
                }
                return Ok(count as i64);
            }
            Err(e) => return Err(e),
        }
        written += n;
    }
    Ok(count as i64)
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

pub const SOCKET_MAX_CAPACITY: usize = 131072;
pub const PIPE_MAX_CAPACITY: usize = 65536;

#[inline]
pub fn append_socket_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut data = snapshot_blob(&blob_id)?;
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    let cur_payload = data.len() - SOCKET_HEADER_LEN;
    if cur_payload >= SOCKET_MAX_CAPACITY {
        return Err(FsBridgeError::WouldBlock);
    }
    let to_write = bytes.len().min(SOCKET_MAX_CAPACITY - cur_payload);
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

pub struct UnixListenerEntry {
    pub path: Vec<u8>,
    pub listener_blob: BlobId,
    pub pending: Vec<BlobId>,
}

pub static UNIX_LISTENERS: Mutex<Vec<UnixListenerEntry>> = Mutex::new(Vec::new());
pub static SOCKET_ANCILLARY_FDS: Mutex<Vec<(BlobId, u32, u32)>> = Mutex::new(Vec::new());

#[inline]
pub fn fs_is_unix_socket_fd(fd: u32, pid: u32) -> bool {
    let Ok(resolved) = resolve_fd(pid, fd) else {
        return false;
    };
    if is_tty_handle(resolved.handle) {
        return false;
    }
    let Ok(entry) = OBJECT_TABLE.get(resolved.handle) else {
        return false;
    };
    is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG)
}

#[inline]
pub fn socket_is_readable(blob_id: BlobId) -> bool {
    if socket_payload_len(blob_id) != 0 {
        return true;
    }
    if SOCKET_ANCILLARY_FDS
        .lock()
        .iter()
        .any(|(b, _, _)| *b == blob_id)
    {
        return true;
    }
    if UNIX_LISTENERS
        .lock()
        .iter()
        .any(|l| l.listener_blob == blob_id && !l.pending.is_empty())
    {
        return true;
    }
    false
}

fn parse_sockaddr_un_path(addr_ptr: u64, addr_len: u64) -> Result<Vec<u8>, FsBridgeError> {
    if addr_ptr == 0 || addr_len < 3 {
        return Err(FsBridgeError::Invalid);
    }
    let copy_len = (addr_len as usize).min(110);
    let mut raw = [0u8; 110];
    copy_from_user(raw.as_mut_ptr(), addr_ptr as *const u8, copy_len)
        .map_err(|_| FsBridgeError::Fault)?;
    let family = u16::from_ne_bytes([raw[0], raw[1]]);
    if family != 1 {
        return Err(FsBridgeError::Invalid);
    }
    let path_slice = &raw[2..copy_len];
    let end = path_slice
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(path_slice.len());
    if end == 0 {
        return Err(FsBridgeError::Invalid);
    }
    resolve_path_with_symlinks(&path_slice[..end], false, true)
}

/// `socket(AF_UNIX, ty, protocol)`.
pub fn fs_unix_socket(domain: i32, ty: i32, protocol: i32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let clean_ty = ty & !((O_NONBLOCK | O_CLOEXEC) as i32);
    if domain != 1 || protocol != 0 || (clean_ty != 1 && clean_ty != 2) {
        return Err(FsBridgeError::Invalid);
    }
    let blob_id = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    BLOB_CACHE
        .insert(blob_id, socket_blob_with_peer(BlobId([0u8; 32])))
        .map_err(exofs_to_bridge_error)?;
    let obj_fd = OBJECT_TABLE
        .open(blob_id, open_flags::O_RDWR, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        let fd_flags = fd_table_flags(ty as u32, open_flags::O_RDWR);
        if let Some(logical) = install_process_fd(pid, obj_fd as u64, fd_flags) {
            return Ok(logical as i64);
        }
        let _ = OBJECT_TABLE.close(obj_fd);
        return Err(FsBridgeError::NoMemory);
    }
    Ok(obj_fd as i64)
}

/// `bind(fd, sockaddr_un*, addrlen)`.
pub fn fs_unix_bind(
    fd: u32,
    addr_ptr: u64,
    addr_len: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let normalized = parse_sockaddr_un_path(addr_ptr, addr_len)?;
    if path_entry(&normalized).is_ok() {
        return Err(FsBridgeError::Exists);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized)?;
    ensure_directory_chain(&parent_path)?;
    let file_blob = blob_id_for_path(&normalized)?;
    BLOB_CACHE
        .insert(file_blob, Vec::new())
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&file_blob);
    upsert_mode(file_blob, 0o140000 | 0o660);
    upsert_parent_entry(&parent_path, &leaf, file_blob, PATH_INDEX_KIND_FILE)?;

    let mut listeners = UNIX_LISTENERS.lock();
    if let Some(existing) = listeners.iter_mut().find(|l| l.path == normalized) {
        existing.listener_blob = entry.blob_id;
        existing.pending.clear();
    } else {
        listeners
            .try_reserve(1)
            .map_err(|_| FsBridgeError::NoMemory)?;
        listeners.push(UnixListenerEntry {
            path: normalized,
            listener_blob: entry.blob_id,
            pending: Vec::new(),
        });
    }
    Ok(0)
}

/// `listen(fd, backlog)`.
pub fn fs_unix_listen(fd: u32, _backlog: i32, pid: u32) -> Result<i64, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    Ok(0)
}

/// `connect(fd, sockaddr_un*, addrlen)`.
pub fn fs_unix_connect(
    fd: u32,
    addr_ptr: u64,
    addr_len: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let normalized = parse_sockaddr_un_path(addr_ptr, addr_len)?;
    let server_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);
    {
        let mut listeners = UNIX_LISTENERS.lock();
        let Some(listener) = listeners.iter_mut().find(|l| l.path == normalized) else {
            return Err(FsBridgeError::NotFound);
        };
        listener
            .pending
            .try_reserve(1)
            .map_err(|_| FsBridgeError::NoMemory)?;
        listener.pending.push(server_blob);
    }
    BLOB_CACHE
        .insert(entry.blob_id, socket_blob_with_peer(server_blob))
        .map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(server_blob, socket_blob_with_peer(entry.blob_id))
        .map_err(exofs_to_bridge_error)?;
    Ok(0)
}

/// `accept(fd, addr_ptr, addr_len_ptr)`.
pub fn fs_unix_accept(
    fd: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let server_blob = {
        let mut listeners = UNIX_LISTENERS.lock();
        let Some(listener) = listeners
            .iter_mut()
            .find(|l| l.listener_blob == entry.blob_id)
        else {
            return Err(FsBridgeError::Invalid);
        };
        if listener.pending.is_empty() {
            return Err(FsBridgeError::WouldBlock);
        }
        listener.pending.remove(0)
    };
    if addr_ptr != 0 && addr_len_ptr != 0 {
        let _ = write_user_typed::<u32>(addr_len_ptr, 2);
        let _ = write_user_typed::<u16>(addr_ptr, 1);
    }
    let s_fd = OBJECT_TABLE
        .open(server_blob, open_flags::O_RDWR, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        let fd_flags = fd_table_flags(0, open_flags::O_RDWR);
        if let Some(logical) = install_process_fd(pid, s_fd as u64, fd_flags) {
            return Ok(logical as i64);
        }
        let _ = OBJECT_TABLE.close(s_fd);
        return Err(FsBridgeError::NoMemory);
    }
    Ok(s_fd as i64)
}

/// `sendto` sur socket `AF_UNIX`.
pub fn fs_unix_sendto(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    _flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let peer = socket_peer_blob(entry.blob_id)?;
    let input = read_user_bytes(buf_ptr, len)?;
    append_socket_payload(peer, &input)
}

/// `recvfrom` sur socket `AF_UNIX`.
pub fn fs_unix_recvfrom(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let consume = (flags & 0x02) == 0;
    let data = read_socket_payload(entry.blob_id, len, consume)?;
    if data.is_empty() {
        return Err(FsBridgeError::WouldBlock);
    }
    copy_to_user(buf_ptr as *mut u8, data.as_ptr(), data.len())
        .map_err(|_| FsBridgeError::Fault)?;
    Ok(data.len() as i64)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UnixMsghdr {
    msg_name: u64,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: i32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UnixCmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

/// `sendmsg` sur socket `AF_UNIX` avec support `SCM_RIGHTS`.
pub fn fs_unix_sendmsg(
    fd: u32,
    msg_ptr: u64,
    _flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if msg_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let peer = socket_peer_blob(entry.blob_id)?;
    let msg = read_user_typed::<UnixMsghdr>(msg_ptr).map_err(|_| FsBridgeError::Fault)?;
    if msg.msg_iovlen > 1024 {
        return Err(FsBridgeError::Invalid);
    }

    // Parse SCM_RIGHTS ancillary FDs if present
    if msg.msg_control != 0 && msg.msg_controllen >= 16 {
        let ctrl_len = (msg.msg_controllen as usize).min(4096);
        let ctrl_bytes = read_user_bytes(msg.msg_control, ctrl_len)?;
        let mut off = 0usize;
        while off + 16 <= ctrl_bytes.len() {
            let hdr = unsafe {
                core::ptr::read_unaligned(ctrl_bytes.as_ptr().add(off) as *const UnixCmsghdr)
            };
            if hdr.cmsg_len < 16 || off + hdr.cmsg_len > ctrl_bytes.len() {
                break;
            }
            if hdr.cmsg_level == 1 && hdr.cmsg_type == 1 {
                let fd_bytes = hdr.cmsg_len - 16;
                let fd_count = fd_bytes / 4;
                for i in 0..fd_count {
                    let pos = off + 16 + i * 4;
                    let raw_fd = i32::from_ne_bytes([
                        ctrl_bytes[pos],
                        ctrl_bytes[pos + 1],
                        ctrl_bytes[pos + 2],
                        ctrl_bytes[pos + 3],
                    ]);
                    if raw_fd >= 0 {
                        if let Ok(src_res) = resolve_fd(pid, raw_fd as u32) {
                            if let Ok(dup_handle) = OBJECT_TABLE.dup(src_res.handle) {
                                let mut q = SOCKET_ANCILLARY_FDS.lock();
                                if q.try_reserve(1).is_ok() {
                                    q.push((peer, dup_handle, src_res.flags));
                                }
                            }
                        }
                    }
                }
            }
            off += (hdr.cmsg_len + 7) & !7;
        }
    }

    let mut payload = Vec::new();
    for i in 0..msg.msg_iovlen {
        let addr = msg
            .msg_iov
            .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        if iov.iov_len != 0 {
            let chunk = read_user_bytes(iov.iov_base, iov.iov_len as usize)?;
            payload
                .try_reserve(chunk.len())
                .map_err(|_| FsBridgeError::NoMemory)?;
            payload.extend_from_slice(&chunk);
        }
    }
    append_socket_payload(peer, &payload)
}

/// `recvmsg` sur socket `AF_UNIX` avec support `SCM_RIGHTS`.
pub fn fs_unix_recvmsg(
    fd: u32,
    msg_ptr: u64,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if msg_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let mut msg = read_user_typed::<UnixMsghdr>(msg_ptr).map_err(|_| FsBridgeError::Fault)?;
    if msg.msg_iovlen > 1024 {
        return Err(FsBridgeError::Invalid);
    }

    let mut total_cap = 0usize;
    for i in 0..msg.msg_iovlen {
        let addr = msg
            .msg_iov
            .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        total_cap = total_cap.saturating_add(iov.iov_len as usize);
    }

    let consume = (flags & 0x02) == 0;
    let data = read_socket_payload(entry.blob_id, total_cap, consume)?;
    let has_ancillary = SOCKET_ANCILLARY_FDS
        .lock()
        .iter()
        .any(|(b, _, _)| *b == entry.blob_id);

    if data.is_empty() && !has_ancillary {
        return Err(FsBridgeError::WouldBlock);
    }

    let mut copied = 0usize;
    for i in 0..msg.msg_iovlen {
        if copied >= data.len() {
            break;
        }
        let addr = msg
            .msg_iov
            .checked_add(i.saturating_mul(size_of::<LinuxIovec>() as u64))
            .ok_or(FsBridgeError::Fault)?;
        let iov = read_user_typed::<LinuxIovec>(addr).map_err(|_| FsBridgeError::Fault)?;
        let chunk = (data.len() - copied).min(iov.iov_len as usize);
        if chunk != 0 {
            copy_to_user(
                iov.iov_base as *mut u8,
                data[copied..copied + chunk].as_ptr(),
                chunk,
            )
            .map_err(|_| FsBridgeError::Fault)?;
            copied += chunk;
        }
    }

    if msg.msg_control != 0 && msg.msg_controllen >= 16 && has_ancillary {
        let max_fds = ((msg.msg_controllen as usize).saturating_sub(16)) / 4;
        let mut received_fds = Vec::new();
        {
            let mut q = SOCKET_ANCILLARY_FDS.lock();
            let mut i = 0;
            while i < q.len() && received_fds.len() < max_fds {
                if q[i].0 == entry.blob_id {
                    let (_, obj_handle, fd_flags) = q.remove(i);
                    let new_flags = if (flags & 0x4000_0000) != 0 {
                        fd_flags | O_CLOEXEC
                    } else {
                        fd_flags
                    };
                    let new_fd = if process_has_fd_table(pid) {
                        install_process_fd(pid, obj_handle as u64, new_flags)
                            .unwrap_or(obj_handle as i32)
                    } else {
                        obj_handle as i32
                    };
                    let _ = received_fds.try_reserve(1);
                    received_fds.push(new_fd);
                } else {
                    i += 1;
                }
            }
        }
        if !received_fds.is_empty() {
            let cmsg_len = 16 + received_fds.len() * 4;
            let aligned_len = ((cmsg_len + 7) & !7).min(msg.msg_controllen as usize);
            let hdr = UnixCmsghdr {
                cmsg_len,
                cmsg_level: 1,
                cmsg_type: 1,
            };
            write_user_typed(msg.msg_control, hdr).map_err(|_| FsBridgeError::Fault)?;
            for (idx, &rfd) in received_fds.iter().enumerate() {
                let fd_addr = msg
                    .msg_control
                    .checked_add(16 + (idx as u64) * 4)
                    .ok_or(FsBridgeError::Fault)?;
                write_user_typed(fd_addr, rfd).map_err(|_| FsBridgeError::Fault)?;
            }
            msg.msg_controllen = aligned_len as u64;
        } else {
            msg.msg_controllen = 0;
        }
    } else {
        msg.msg_controllen = 0;
    }
    msg.msg_flags = 0;
    write_user_typed(msg_ptr, msg).map_err(|_| FsBridgeError::Fault)?;
    Ok(copied as i64)
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

