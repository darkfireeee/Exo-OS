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

/// FIX-V8: log E9 console mode only once before tty_server.
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

    pub fn push_slice(&mut self, bytes: &[u8]) -> usize {
        if self.head > 0 && self.len > 0 {
            self.data.copy_within(self.head..self.head + self.len, 0);
        }
        self.head = 0;
        let space = TTY_LINE_MAX.saturating_sub(self.len);
        let n = bytes.len().min(space);
        if n != 0 {
            self.data[self.len..self.len + n].copy_from_slice(&bytes[..n]);
            self.len += n;
        }
        n
    }

    pub fn peek_into(&self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        if n != 0 {
            out[..n].copy_from_slice(&self.data[self.head..self.head + n]);
        }
        n
    }

    pub fn consume(&mut self, n: usize) {
        let taken = n.min(self.len);
        self.head += taken;
        self.len -= taken;
        if self.len == 0 {
            self.head = 0;
        }
    }

    pub fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let n = self.peek_into(out);
        self.consume(n);
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
    // Headless console tap: mirror console output to debug port E9 only during debug trace.
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

/// TTY write without heap allocation: copies user memory slices
/// directly into stack-allocated TTY line requests (184 bytes max).
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

static TTY_REFILL_LOCK: Mutex<()> = Mutex::new(());

fn tty_refill_stdin_locked(pid: u32) -> Result<(), FsBridgeError> {
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

pub fn tty_refill_stdin(pid: u32) -> Result<(), FsBridgeError> {
    let _guard = TTY_REFILL_LOCK.lock();
    tty_refill_stdin_locked(pid)
}

pub fn tty_read_bytes(buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if count == 0 {
        return Ok(0);
    }
    let _guard = TTY_REFILL_LOCK.lock();
    let mut out = [0u8; TTY_LINE_MAX];
    let cap = count.min(TTY_LINE_MAX);
    let mut copied = TTY_STDIN.lock().peek_into(&mut out[..cap]);
    if copied == 0 {
        tty_refill_stdin_locked(pid)?;
        copied = TTY_STDIN.lock().peek_into(&mut out[..cap]);
    }
    if copied == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    copy_to_user(buf_ptr as *mut u8, out.as_ptr(), copied).map_err(|_| FsBridgeError::Fault)?;
    TTY_STDIN.lock().consume(copied);
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

static PIPE_PAYLOAD_LOCK: Mutex<()> = Mutex::new(());
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
    Ok(size_of::<u64>() as i64)
}

pub static SOCKET_PAYLOAD_LOCK: Mutex<()> = Mutex::new(());
pub static CLOSED_SOCKETS: Mutex<Vec<BlobId>> = Mutex::new(Vec::new());

#[inline]
fn parse_socket_peer_bytes(data: &[u8]) -> Result<BlobId, FsBridgeError> {
    if data.len() < SOCKET_HEADER_LEN {
        return Err(FsBridgeError::Invalid);
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&data[..SOCKET_HEADER_LEN]);
    Ok(BlobId(bytes))
}

#[inline]
pub fn is_socket_closed(blob_id: BlobId) -> bool {
    CLOSED_SOCKETS.lock().iter().any(|b| *b == blob_id)
}

#[inline]
fn is_socket_peer_closed_locked(blob_id: BlobId) -> bool {
    let Ok(data) = snapshot_blob(&blob_id) else {
        return false;
    };
    let Ok(peer) = parse_socket_peer_bytes(&data) else {
        return false;
    };
    if peer == BlobId([0u8; 32]) {
        return false;
    }
    is_socket_closed(peer)
}

#[inline]
pub fn is_socket_peer_closed(blob_id: BlobId) -> bool {
    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    is_socket_peer_closed_locked(blob_id)
}

/// Clean up kernel-side pseudo-blob state once `OBJECT_TABLE.open_count_for(&blob_id) == 0`.
pub fn cleanup_closed_blob(blob_id: BlobId) {
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return;
    }
    if is_pseudo_blob(&blob_id, PSEUDO_SOCKET_TAG) {
        on_socket_blob_closed(blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_EPOLL_TAG) {
        remove_epoll_watches_for_blob(blob_id);
        BLOB_CACHE.invalidate(&blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_PIPE_TAG)
        || is_pseudo_blob(&blob_id, PSEUDO_EVENTFD_TAG)
        || is_pseudo_blob(&blob_id, PSEUDO_INOTIFY_TAG)
    {
        BLOB_CACHE.invalidate(&blob_id);
    }
}

pub fn on_socket_blob_closed(blob_id: BlobId) {
    let mut to_close = Vec::new();
    let _ = to_close.try_reserve(1);
    to_close.push(blob_id);
    {
        let mut listeners = UNIX_LISTENERS.lock();
        let mut i = 0;
        while i < listeners.len() {
            if listeners[i].listener_blob == blob_id {
                let removed = listeners.swap_remove(i);
                for pending in removed.pending {
                    if to_close.try_reserve(1).is_ok() {
                        to_close.push(pending);
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    let mut ancillary_handles = Vec::new();
    {
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        for &s_blob in &to_close {
            let peer = snapshot_blob(&s_blob)
                .ok()
                .and_then(|d| parse_socket_peer_bytes(&d).ok())
                .unwrap_or(BlobId([0u8; 32]));
            BLOB_CACHE.invalidate(&s_blob);

            {
                let mut closed = CLOSED_SOCKETS.lock();
                if let Some(pos) = closed.iter().position(|b| *b == s_blob) {
                    closed.swap_remove(pos);
                }
                if peer != BlobId([0u8; 32]) {
                    if let Some(peer_pos) = closed.iter().position(|b| *b == peer) {
                        // Both endpoints are closed; evict peer instead of growing CLOSED_SOCKETS.
                        closed.swap_remove(peer_pos);
                    } else if closed.try_reserve(1).is_ok() {
                        closed.push(s_blob);
                    }
                }
            }

            {
                let mut q = SOCKET_ANCILLARY_FDS.lock();
                let mut i = 0;
                while i < q.len() {
                    if q[i].0 == s_blob {
                        let (_, dup_handle, _) = q.remove(i);
                        if ancillary_handles.try_reserve(1).is_ok() {
                            ancillary_handles.push(dup_handle);
                        } else {
                            let _ = OBJECT_TABLE.close(dup_handle);
                        }
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    for h in ancillary_handles {
        let maybe_blob = OBJECT_TABLE.get(h).ok().map(|e| e.blob_id);
        if OBJECT_TABLE.close(h) {
            if let Some(b) = maybe_blob {
                cleanup_closed_blob(b);
            }
        }
    }
}

#[inline]
pub fn socket_blob_with_peer(peer: BlobId) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(peer.as_bytes());
    data
}

#[inline]
pub fn socket_peer_blob(blob_id: BlobId) -> Result<BlobId, FsBridgeError> {
    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    let data = snapshot_blob(&blob_id)?;
    parse_socket_peer_bytes(&data)
}

#[inline]
pub fn socket_payload_len(blob_id: BlobId) -> usize {
    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    BLOB_CACHE
        .get(&blob_id)
        .map(|data| data.len().saturating_sub(SOCKET_HEADER_LEN))
        .unwrap_or(0)
}

#[inline]
fn read_socket_payload_locked(
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
pub fn read_socket_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    read_socket_payload_locked(blob_id, count, consume)
}

/// Read up to `count` bytes from socket `blob_id` into user `buf_ptr`, consuming only after copy succeeds.
pub fn socket_read_to_user(
    blob_id: BlobId,
    buf_ptr: u64,
    count: usize,
    consume: bool,
    nonblock: bool,
) -> Result<i64, FsBridgeError> {
    if count == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    loop {
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        let data = snapshot_blob(&blob_id)?;
        if data.len() > SOCKET_HEADER_LEN {
            let read_len = count.min(data.len() - SOCKET_HEADER_LEN);
            let out = &data[SOCKET_HEADER_LEN..SOCKET_HEADER_LEN + read_len];
            copy_to_user(buf_ptr as *mut u8, out.as_ptr(), read_len)
                .map_err(|_| FsBridgeError::Fault)?;
            if consume {
                let mut next = data[..SOCKET_HEADER_LEN].to_vec();
                next.extend_from_slice(&data[SOCKET_HEADER_LEN + read_len..]);
                BLOB_CACHE
                    .insert(blob_id, next)
                    .map_err(exofs_to_bridge_error)?;
                let _ = BLOB_CACHE.mark_dirty(&blob_id);
            }
            return Ok(read_len as i64);
        }
        let peer = parse_socket_peer_bytes(&data)?;
        let peer_closed = peer != BlobId([0u8; 32]) && is_socket_closed(peer);
        drop(_guard);

        if peer_closed {
            return Ok(0);
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
    }
}

pub const SOCKET_MAX_CAPACITY: usize = 131072;
pub const PIPE_MAX_CAPACITY: usize = 65536;
const MAX_ANCILLARY_FDS_PER_SOCKET: usize = 256;

#[inline]
fn append_socket_payload_locked(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    if blob_id == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if is_socket_closed(blob_id) {
        return Err(FsBridgeError::BrokenPipe);
    }
    if bytes.is_empty() {
        let _ = snapshot_blob(&blob_id)?;
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
pub fn append_socket_payload(blob_id: BlobId, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    append_socket_payload_locked(blob_id, bytes)
}

#[inline]
pub fn read_pipe_payload(
    blob_id: BlobId,
    count: usize,
    consume: bool,
) -> Result<Vec<u8>, FsBridgeError> {
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
                return Ok(read_len as i64);
            }
        }
        if OBJECT_TABLE.open_count_for(&blob_id) <= OBJECT_TABLE.readable_count_for(&blob_id) {
            return Ok(0);
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
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
    let _guard = PIPE_PAYLOAD_LOCK.lock();
    append_pipe_payload_locked(blob_id, bytes)
}

/// Write `input` bytes to pipe `blob_id`, checking `SIGPIPE` and `PIPE_MAX_CAPACITY`.
pub fn write_pipe_bytes(
    blob_id: BlobId,
    input: &[u8],
    nonblock: bool,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    loop {
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
                if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
                    return Err(FsBridgeError::Interrupted);
                }
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
    if is_socket_peer_closed(blob_id) {
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

    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    if is_socket_closed(entry.blob_id) {
        return Err(FsBridgeError::Invalid);
    }
    let cur_data = snapshot_blob(&entry.blob_id)?;
    let cur_peer = parse_socket_peer_bytes(&cur_data)?;
    if cur_peer != BlobId([0u8; 32]) {
        return Err(FsBridgeError::Invalid);
    }

    let mut listeners = UNIX_LISTENERS.lock();
    if listeners.iter().any(|l| l.listener_blob == entry.blob_id) {
        return Err(FsBridgeError::Invalid);
    }
    let Some(listener) = listeners.iter_mut().find(|l| l.path == normalized) else {
        return Err(FsBridgeError::NotFound);
    };
    listener
        .pending
        .try_reserve(1)
        .map_err(|_| FsBridgeError::NoMemory)?;

    if let Err(err) = BLOB_CACHE.insert(server_blob, socket_blob_with_peer(entry.blob_id)) {
        return Err(exofs_to_bridge_error(err));
    }
    if let Err(err) = BLOB_CACHE.insert(entry.blob_id, socket_blob_with_peer(server_blob)) {
        BLOB_CACHE.invalidate(&server_blob);
        return Err(exofs_to_bridge_error(err));
    }
    listener.pending.push(server_blob);
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
    let nonblock = (resolved.flags & O_NONBLOCK) != 0;
    let server_blob = loop {
        {
            let mut listeners = UNIX_LISTENERS.lock();
            let Some(listener) = listeners
                .iter_mut()
                .find(|l| l.listener_blob == entry.blob_id)
            else {
                return Err(FsBridgeError::Invalid);
            };
            if !listener.pending.is_empty() {
                break listener.pending.remove(0);
            }
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
            return Err(FsBridgeError::Interrupted);
        }
    };
    if addr_ptr != 0 && addr_len_ptr != 0 {
        let _ = write_user_typed::<u32>(addr_len_ptr, 2);
        let _ = write_user_typed::<u16>(addr_ptr, 1);
    }
    let s_fd = OBJECT_TABLE
        .open(server_blob, open_flags::O_RDWR, 0, 0, 0)
        .map_err(exofs_to_bridge_error)?;
    if process_has_fd_table(pid) {
        let fd_flags = fd_table_flags(0, open_flags::O_RDWR);
        if let Some(logical) = install_process_fd(pid, s_fd as u64, fd_flags) {
            return Ok(logical as i64);
        }
        let _ = close_object_handle_with_cleanup(s_fd);
        return Err(FsBridgeError::NoMemory);
    }
    Ok(s_fd as i64)
}

/// `sendto` on `AF_UNIX` socket.
pub fn fs_unix_sendto(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    _flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if len != 0 && buf_ptr == 0 {
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
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if len == 0 {
        return append_socket_payload(peer, &[]);
    }
    let input = read_user_bytes(buf_ptr, len)?;
    append_socket_payload(peer, &input)
}

/// `recvfrom` on `AF_UNIX` socket.
pub fn fs_unix_recvfrom(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let consume = (flags & 0x02) == 0;
    let nonblock = (resolved.flags & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    socket_read_to_user(entry.blob_id, buf_ptr, len, consume, nonblock)
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

fn rollback_staged_ancillary(staged: &[(BlobId, u32, u32)]) {
    for &(_, h, _) in staged {
        let _ = close_object_handle_with_cleanup(h);
    }
}

fn rollback_received_fds(pid: u32, received: &[(i32, u32)], has_fd_table: bool) {
    for &(rfd, obj_handle) in received {
        if has_fd_table {
            if let Some(h) = close_process_fd(pid, rfd as u32) {
                if h <= u32::MAX as u64 {
                    let _ = close_object_handle_with_cleanup(h as u32);
                }
            } else {
                let _ = close_object_handle_with_cleanup(obj_handle);
            }
        } else {
            let _ = close_object_handle_with_cleanup(obj_handle);
        }
    }
}

/// `sendmsg` on `AF_UNIX` socket with `SCM_RIGHTS` support.
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
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if is_socket_closed(peer) {
        return Err(FsBridgeError::BrokenPipe);
    }
    let msg = read_user_typed::<UnixMsghdr>(msg_ptr).map_err(|_| FsBridgeError::Fault)?;
    if msg.msg_iovlen > 1024 {
        return Err(FsBridgeError::Invalid);
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

    // Validate and duplicate all SCM_RIGHTS ancillary FDs atomically before queuing.
    let mut staged_fds: Vec<(BlobId, u32, u32)> = Vec::new();
    if msg.msg_control != 0 && msg.msg_controllen >= 16 {
        let ctrl_len = (msg.msg_controllen as usize).min(4096);
        let ctrl_bytes = read_user_bytes(msg.msg_control, ctrl_len)?;
        let mut off = 0usize;
        while off + 16 <= ctrl_bytes.len() {
            let hdr = unsafe {
                core::ptr::read_unaligned(ctrl_bytes.as_ptr().add(off) as *const UnixCmsghdr)
            };
            if hdr.cmsg_len < 16 || off + hdr.cmsg_len > ctrl_bytes.len() {
                rollback_staged_ancillary(&staged_fds);
                return Err(FsBridgeError::Invalid);
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
                    if raw_fd < 0 {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    }
                    let Ok(src_res) = resolve_fd(pid, raw_fd as u32) else {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    };
                    let Ok(dup_handle) = OBJECT_TABLE.dup(src_res.handle) else {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    };
                    if staged_fds.try_reserve(1).is_err() {
                        let _ = close_object_handle_with_cleanup(dup_handle);
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::NoMemory);
                    }
                    staged_fds.push((peer, dup_handle, src_res.flags));
                }
            }
            off += (hdr.cmsg_len + 7) & !7;
        }
    }

    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    let written = if staged_fds.is_empty() {
        append_socket_payload_locked(peer, &payload)?
    } else {
        let mut q = SOCKET_ANCILLARY_FDS.lock();
        let existing = q.iter().filter(|(b, _, _)| *b == peer).count();
        if existing + staged_fds.len() > MAX_ANCILLARY_FDS_PER_SOCKET {
            drop(q);
            rollback_staged_ancillary(&staged_fds);
            return Err(FsBridgeError::WouldBlock);
        }
        if q.try_reserve(staged_fds.len()).is_err() {
            drop(q);
            rollback_staged_ancillary(&staged_fds);
            return Err(FsBridgeError::NoMemory);
        }
        let n = match append_socket_payload_locked(peer, &payload) {
            Ok(n) => n,
            Err(err) => {
                drop(q);
                rollback_staged_ancillary(&staged_fds);
                return Err(err);
            }
        };
        q.extend(staged_fds);
        n
    };
    Ok(written)
}

fn fail_recvmsg(
    guard: spin::mutex::MutexGuard<'_, ()>,
    pid: u32,
    received: &[(i32, u32)],
    has_fd_table: bool,
    err: FsBridgeError,
) -> FsBridgeError {
    drop(guard);
    rollback_received_fds(pid, received, has_fd_table);
    err
}

/// `recvmsg` on `AF_UNIX` socket with `SCM_RIGHTS` support.
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
    let nonblock = (resolved.flags & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    loop {
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        let data = read_socket_payload_locked(entry.blob_id, total_cap, false)?;
        let matching: Vec<(u32, u32)> = {
            let q = SOCKET_ANCILLARY_FDS.lock();
            q.iter()
                .filter(|(b, _, _)| *b == entry.blob_id)
                .map(|&(_, h, f)| (h, f))
                .collect()
        };
        if data.is_empty() && matching.is_empty() {
            let peer_closed = is_socket_peer_closed_locked(entry.blob_id);
            drop(_guard);
            if peer_closed {
                msg.msg_controllen = 0;
                msg.msg_flags = 0;
                write_user_typed(msg_ptr, msg).map_err(|_| FsBridgeError::Fault)?;
                return Ok(0);
            }
            if nonblock {
                return Err(FsBridgeError::WouldBlock);
            }
            if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
                return Err(FsBridgeError::Interrupted);
            }
            continue;
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

        let has_fd_table = process_has_fd_table(pid);
        let mut received_fds: Vec<(i32, u32)> = Vec::new();
        let mut out_msg_flags: i32 = 0;
        if !matching.is_empty() {
            let max_fds = if msg.msg_control != 0 && msg.msg_controllen >= 16 {
                ((msg.msg_controllen as usize).saturating_sub(16)) / 4
            } else {
                0
            };
            let take = matching.len().min(max_fds);
            if matching.len() > take {
                out_msg_flags |= 0x08; // MSG_CTRUNC
            }
            for &(obj_handle, fd_flags) in matching.iter().take(take) {
                let Ok(dup_handle) = OBJECT_TABLE.dup(obj_handle) else {
                    return Err(fail_recvmsg(_guard, pid, &received_fds, has_fd_table, FsBridgeError::NoMemory));
                };
                let new_flags = if (flags & 0x4000_0000) != 0 {
                    fd_flags | O_CLOEXEC
                } else {
                    fd_flags
                };
                let new_fd = if has_fd_table {
                    let Some(installed) = install_process_fd(pid, dup_handle as u64, new_flags)
                    else {
                        drop(_guard);
                        let _ = close_object_handle_with_cleanup(dup_handle);
                        rollback_received_fds(pid, &received_fds, has_fd_table);
                        return Err(FsBridgeError::NoMemory);
                    };
                    installed
                } else {
                    dup_handle as i32
                };
                if received_fds.try_reserve(1).is_err() {
                    drop(_guard);
                    rollback_received_fds(pid, &[(new_fd, dup_handle)], has_fd_table);
                    rollback_received_fds(pid, &received_fds, has_fd_table);
                    return Err(FsBridgeError::NoMemory);
                }
                received_fds.push((new_fd, dup_handle));
            }

            if !received_fds.is_empty() {
                let cmsg_len = 16 + received_fds.len() * 4;
                let aligned_len = ((cmsg_len + 7) & !7).min(msg.msg_controllen as usize);
                let hdr = UnixCmsghdr {
                    cmsg_len,
                    cmsg_level: 1,
                    cmsg_type: 1,
                };
                if write_user_typed(msg.msg_control, hdr).is_err() {
                    return Err(fail_recvmsg(_guard, pid, &received_fds, has_fd_table, FsBridgeError::Fault));
                }
                for (idx, &(rfd, _)) in received_fds.iter().enumerate() {
                    let Some(fd_addr) = msg.msg_control.checked_add(16 + (idx as u64) * 4) else {
                        return Err(fail_recvmsg(_guard, pid, &received_fds, has_fd_table, FsBridgeError::Fault));
                    };
                    if write_user_typed(fd_addr, rfd).is_err() {
                        return Err(fail_recvmsg(_guard, pid, &received_fds, has_fd_table, FsBridgeError::Fault));
                    }
                }
                msg.msg_controllen = aligned_len as u64;
            } else {
                msg.msg_controllen = 0;
            }
        } else {
            msg.msg_controllen = 0;
        }

        msg.msg_flags = out_msg_flags;
        if write_user_typed(msg_ptr, msg).is_err() {
            return Err(fail_recvmsg(_guard, pid, &received_fds, has_fd_table, FsBridgeError::Fault));
        }

        if consume {
            if copied != 0 {
                let _ = read_socket_payload_locked(entry.blob_id, copied, true)?;
            }
            if !matching.is_empty() {
                let mut to_close = Vec::new();
                {
                    let mut q = SOCKET_ANCILLARY_FDS.lock();
                    let mut i = 0;
                    let mut removed = 0usize;
                    while i < q.len() && removed < matching.len() {
                        if q[i].0 == entry.blob_id {
                            let (_, obj_handle, _) = q.remove(i);
                            if to_close.try_reserve(1).is_ok() {
                                to_close.push(obj_handle);
                            } else {
                                let _ = OBJECT_TABLE.close(obj_handle);
                            }
                            removed += 1;
                        } else {
                            i += 1;
                        }
                    }
                }
                drop(_guard);
                for h in to_close {
                    let _ = close_object_handle_with_cleanup(h);
                }
            }
        }
        return Ok(copied as i64);
    }
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

