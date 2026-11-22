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

pub struct UnixSocketMeta {
    pub blob_id: BlobId,
    pub sock_type: i32,
    pub bound_path: Vec<u8>,
    pub peer_path: Vec<u8>,
    pub shutdown_rd: bool,
    pub shutdown_wr: bool,
    pub peer_shutdown_wr: bool,
    pub cred_pid: u32,
    pub cred_uid: u32,
    pub cred_gid: u32,
    pub last_error: i32,
    pub passcred: bool,
    pub reuseaddr: bool,
    pub rcvbuf: u32,
    pub sndbuf: u32,
}

impl UnixSocketMeta {
    fn new(blob_id: BlobId, sock_type: i32, pid: u32) -> Self {
        let (uid, gid) = process_uid_gid(pid);
        Self {
            blob_id,
            sock_type,
            bound_path: Vec::new(),
            peer_path: Vec::new(),
            shutdown_rd: false,
            shutdown_wr: false,
            peer_shutdown_wr: false,
            cred_pid: pid,
            cred_uid: uid,
            cred_gid: gid,
            last_error: 0,
            passcred: false,
            reuseaddr: false,
            rcvbuf: SOCKET_MAX_CAPACITY as u32,
            sndbuf: SOCKET_MAX_CAPACITY as u32,
        }
    }
}

pub static UNIX_SOCKET_META: Mutex<Vec<UnixSocketMeta>> = Mutex::new(Vec::new());

fn process_uid_gid(pid: u32) -> (u32, u32) {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| {
            let c = pcb.creds.lock();
            (c.euid, c.egid)
        })
        .unwrap_or((0, 0))
}

fn register_unix_socket_meta(blob_id: BlobId, sock_type: i32, pid: u32) {
    let mut table = UNIX_SOCKET_META.lock();
    if let Some(m) = table.iter_mut().find(|m| m.blob_id == blob_id) {
        m.sock_type = sock_type;
        m.cred_pid = pid;
        let (uid, gid) = process_uid_gid(pid);
        m.cred_uid = uid;
        m.cred_gid = gid;
        return;
    }
    if table.try_reserve(1).is_ok() {
        table.push(UnixSocketMeta::new(blob_id, sock_type, pid));
    }
}

fn remove_unix_socket_meta(blob_id: BlobId) {
    let mut table = UNIX_SOCKET_META.lock();
    if let Some(pos) = table.iter().position(|m| m.blob_id == blob_id) {
        table.swap_remove(pos);
    }
}

#[inline]
pub fn socket_is_read_shutdown(blob_id: BlobId) -> bool {
    UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == blob_id)
        .map(|m| m.shutdown_rd || m.peer_shutdown_wr)
        .unwrap_or(false)
}

#[inline]
pub fn socket_is_write_shutdown(blob_id: BlobId) -> bool {
    UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == blob_id)
        .map(|m| m.shutdown_wr)
        .unwrap_or(false)
}

/// Clean up kernel-side pseudo-blob state (or POSIX deferred-unlinked regular files)
/// once `OBJECT_TABLE.open_count_for(&blob_id) == 0`.
pub fn cleanup_closed_blob(blob_id: BlobId) {
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return;
    }
    remove_blob_edge(&blob_id);
    if is_pseudo_blob(&blob_id, PSEUDO_SOCKET_TAG) {
        on_socket_blob_closed(blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_NET_SOCKET_TAG) {
        crate::syscall::net_bridge::on_net_socket_blob_closed(blob_id);
        BLOB_CACHE.invalidate(&blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_EPOLL_TAG) {
        remove_epoll_watches_for_blob(blob_id);
        BLOB_CACHE.invalidate(&blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_MEMFD_TAG) {
        remove_memfd_seals(&blob_id);
        BLOB_CACHE.invalidate(&blob_id);
    } else if is_pseudo_blob(&blob_id, PSEUDO_PIPE_TAG)
        || is_pseudo_blob(&blob_id, PSEUDO_EVENTFD_TAG)
        || is_pseudo_blob(&blob_id, PSEUDO_INOTIFY_TAG)
    {
        BLOB_CACHE.invalidate(&blob_id);
    } else if let Some(is_symlink) = take_deferred_unlink(&blob_id) {
        if is_symlink {
            invalidate_symlink(&blob_id_to_object_id(blob_id));
        }
        BLOB_CACHE.invalidate(&blob_id);
        remove_mode(&blob_id);
        remove_times(&blob_id);
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
    {
        let mut dgram_eps = UNIX_DGRAM_ENDPOINTS.lock();
        let mut i = 0;
        while i < dgram_eps.len() {
            if dgram_eps[i].1 == blob_id {
                dgram_eps.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
    {
        let mut dgram_qs = UNIX_DGRAM_QUEUES.lock();
        let mut i = 0;
        while i < dgram_qs.len() {
            if dgram_qs[i].0 == blob_id {
                dgram_qs.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    let mut ancillary_handles = Vec::new();
    let mut peers_to_wake = Vec::new();
    {
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        for &s_blob in &to_close {
            let peer = snapshot_blob(&s_blob)
                .ok()
                .and_then(|d| parse_socket_peer_bytes(&d).ok())
                .unwrap_or(BlobId([0u8; 32]));
            BLOB_CACHE.invalidate(&s_blob);
            remove_unix_socket_meta(s_blob);

            {
                let mut closed = CLOSED_SOCKETS.lock();
                if let Some(pos) = closed.iter().position(|b| *b == s_blob) {
                    closed.swap_remove(pos);
                }
                if peer != BlobId([0u8; 32]) {
                    if let Some(peer_pos) = closed.iter().position(|b| *b == peer) {
                        // Both endpoints are closed; evict peer instead of growing CLOSED_SOCKETS.
                        closed.swap_remove(peer_pos);
                    } else {
                        if closed.try_reserve(1).is_ok() {
                            closed.push(s_blob);
                        }
                        if peers_to_wake.try_reserve(1).is_ok() {
                            peers_to_wake.push(peer);
                        }
                    }
                }
            }

            {
                let mut q = SOCKET_ANCILLARY_FDS.lock();
                let mut i = 0;
                while i < q.len() {
                    if q[i].0 == s_blob {
                        let (_, dup_handle, _, _) = q.remove(i);
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
    for peer in peers_to_wake {
        notify_io_event(Some(peer));
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
    if socket_is_dgram(blob_id) {
        return dgram_read_to_user(blob_id, buf_ptr, count, consume, nonblock, 0, 0, 0);
    }
    if count == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    loop {
        let gen = io_wait_gen();
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        let data = snapshot_blob(&blob_id)?;
        let peer = parse_socket_peer_bytes(&data)?;
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
                drop(_guard);
                if peer != BlobId([0u8; 32]) {
                    notify_io_event(Some(peer));
                }
            }
            return Ok(read_len as i64);
        }
        let peer_closed = (peer != BlobId([0u8; 32]) && is_socket_closed(peer))
            || socket_is_read_shutdown(blob_id);
        drop(_guard);

        if peer_closed {
            return Ok(0);
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
    }
}

pub const SOCKET_MAX_CAPACITY: usize = 131072;
pub const PIPE_MAX_CAPACITY: usize = 65536;
/// S3 strict inflight ancillary FD quotas (anti-DoS / anti-FD-exhaustion).
pub const MAX_INFLIGHT_FDS_PER_SOCKET: usize = 16;
pub const MAX_INFLIGHT_FDS_PER_PID: usize = 64;
const MAX_DGRAM_QUEUE_PACKETS: usize = 64;
const MAX_DGRAM_PACKET_SIZE: usize = 65536;

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
    let written = {
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        append_socket_payload_locked(blob_id, bytes)?
    };
    if written > 0 {
        notify_io_event(Some(blob_id));
    }
    Ok(written)
}

/// Write `input` to a connected `AF_UNIX` `SOCK_STREAM` or `SOCK_DGRAM` socket,
/// blocking on `IO_WAIT_HUB` if the target buffer is full and `!nonblock`.
pub fn write_unix_socket_bytes(
    self_blob: BlobId,
    input: &[u8],
    flags: u32,
    nonblock: bool,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    let nosignal = (flags & 0x4000) != 0;
    if socket_is_write_shutdown(self_blob) {
        if !nosignal {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        return Err(FsBridgeError::BrokenPipe);
    }
    let peer = socket_peer_blob(self_blob)?;
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if socket_is_dgram(self_blob) {
        return dgram_send_to_blob(self_blob, peer, input, nonblock, nosignal, pid);
    }
    if input.is_empty() {
        return append_socket_payload(peer, &[]);
    }
    loop {
        let gen = io_wait_gen();
        if socket_is_write_shutdown(self_blob) || is_socket_closed(peer) {
            if !nosignal {
                let _ = crate::process::signal::delivery::send_signal_to_pid(
                    crate::process::core::pid::Pid(pid),
                    crate::process::signal::default::Signal::SIGPIPE,
                );
            }
            return Err(FsBridgeError::BrokenPipe);
        }
        match append_socket_payload(peer, input) {
            Ok(n) => return Ok(n),
            Err(FsBridgeError::BrokenPipe) => {
                if !nosignal {
                    let _ = crate::process::signal::delivery::send_signal_to_pid(
                        crate::process::core::pid::Pid(pid),
                        crate::process::signal::default::Signal::SIGPIPE,
                    );
                }
                return Err(FsBridgeError::BrokenPipe);
            }
            Err(FsBridgeError::WouldBlock) if !nonblock => {
                let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
            }
            Err(e) => return Err(e),
        }
    }
}

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
    register_unix_socket_meta(a_blob, clean_ty, pid);
    register_unix_socket_meta(b_blob, clean_ty, pid);

    let obj_flags = open_flags::O_RDWR | ((ty as u32) & O_NONBLOCK);
    let a_fd = OBJECT_TABLE
        .open(a_blob, obj_flags, 0, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    let b_fd = match OBJECT_TABLE.open(b_blob, obj_flags, 0, 0, pid as u64) {
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
    pub listening: bool,
    pub backlog: usize,
}

pub struct UnixDgramPacket {
    pub sender_path: Vec<u8>,
    pub data: Vec<u8>,
}

pub static UNIX_LISTENERS: Mutex<Vec<UnixListenerEntry>> = Mutex::new(Vec::new());
pub static UNIX_DGRAM_ENDPOINTS: Mutex<Vec<(Vec<u8>, BlobId)>> = Mutex::new(Vec::new());
pub static UNIX_DGRAM_QUEUES: Mutex<Vec<(BlobId, Vec<UnixDgramPacket>)>> = Mutex::new(Vec::new());
/// Queued `SCM_RIGHTS` FDs: `(target_socket_blob, dup_obj_handle, fd_flags, sender_pid)`.
pub static SOCKET_ANCILLARY_FDS: Mutex<Vec<(BlobId, u32, u32, u32)>> = Mutex::new(Vec::new());

#[inline]
pub fn socket_is_dgram(blob_id: BlobId) -> bool {
    UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == blob_id)
        .map(|m| m.sock_type == 2)
        .unwrap_or(false)
}

#[inline]
fn dgram_queue_len(blob_id: BlobId) -> usize {
    UNIX_DGRAM_QUEUES
        .lock()
        .iter()
        .find(|(b, _)| *b == blob_id)
        .map(|(_, q)| q.len())
        .unwrap_or(0)
}

#[inline]
pub fn dgram_next_packet_len(blob_id: BlobId) -> usize {
    UNIX_DGRAM_QUEUES
        .lock()
        .iter()
        .find(|(b, _)| *b == blob_id)
        .and_then(|(_, q)| q.first().map(|p| p.data.len()))
        .unwrap_or(0)
}

fn dgram_send_to_blob(
    self_blob: BlobId,
    target_blob: BlobId,
    data: &[u8],
    nonblock: bool,
    nosignal: bool,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if data.len() > MAX_DGRAM_PACKET_SIZE {
        return Err(FsBridgeError::MsgSize);
    }
    if target_blob == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    let sender_path = UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == self_blob)
        .map(|m| m.bound_path.clone())
        .unwrap_or_default();
    loop {
        let gen = io_wait_gen();
        if is_socket_closed(target_blob) {
            if !nosignal {
                let _ = crate::process::signal::delivery::send_signal_to_pid(
                    crate::process::core::pid::Pid(pid),
                    crate::process::signal::default::Signal::SIGPIPE,
                );
            }
            return Err(FsBridgeError::ConnRefused);
        }
        let mut qs = UNIX_DGRAM_QUEUES.lock();
        let queue = if let Some((_, q)) = qs.iter_mut().find(|(b, _)| *b == target_blob) {
            q
        } else {
            qs.try_reserve(1).map_err(|_| FsBridgeError::NoMemory)?;
            qs.push((target_blob, Vec::new()));
            &mut qs.last_mut().unwrap().1
        };
        if queue.len() < MAX_DGRAM_QUEUE_PACKETS {
            queue.try_reserve(1).map_err(|_| FsBridgeError::NoMemory)?;
            let mut pkt_data = Vec::new();
            pkt_data
                .try_reserve_exact(data.len())
                .map_err(|_| FsBridgeError::NoMemory)?;
            pkt_data.extend_from_slice(data);
            queue.push(UnixDgramPacket {
                sender_path,
                data: pkt_data,
            });
            drop(qs);
            notify_io_event(Some(target_blob));
            return Ok(data.len() as i64);
        }
        drop(qs);
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
    }
}

fn dgram_read_to_user(
    blob_id: BlobId,
    buf_ptr: u64,
    count: usize,
    consume: bool,
    nonblock: bool,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, FsBridgeError> {
    if count != 0 && buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    loop {
        let gen = io_wait_gen();
        let pkt_opt = {
            let mut qs = UNIX_DGRAM_QUEUES.lock();
            if let Some((_, q)) = qs.iter_mut().find(|(b, _)| *b == blob_id) {
                if !q.is_empty() {
                    if consume {
                        Some(q.remove(0))
                    } else {
                        Some(UnixDgramPacket {
                            sender_path: q[0].sender_path.clone(),
                            data: q[0].data.clone(),
                        })
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(pkt) = pkt_opt {
            let full_len = pkt.data.len();
            let copy_len = full_len.min(count);
            if copy_len != 0 {
                if copy_to_user(buf_ptr as *mut u8, pkt.data.as_ptr(), copy_len).is_err() {
                    if consume {
                        let mut qs = UNIX_DGRAM_QUEUES.lock();
                        if let Some((_, q)) = qs.iter_mut().find(|(b, _)| *b == blob_id) {
                            q.insert(0, pkt);
                        }
                    }
                    return Err(FsBridgeError::Fault);
                }
            }
            if addr_ptr != 0 && addr_len_ptr != 0 {
                write_sockaddr_un(addr_ptr, addr_len_ptr, &pkt.sender_path)?;
            }
            if consume {
                notify_io_event(Some(blob_id));
            }
            let reported = if (flags & 0x20) != 0 { full_len } else { copy_len };
            return Ok(reported as i64);
        }
        if socket_is_read_shutdown(blob_id) {
            return Ok(0);
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
    }
}

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

pub fn socket_readiness_mask(blob_id: BlobId) -> u32 {
    let mut mask = 0u32;

    // Listening SOCK_STREAM socket: readable iff pending connection queue is non-empty.
    {
        let listeners = UNIX_LISTENERS.lock();
        if let Some(l) = listeners.iter().find(|l| l.listener_blob == blob_id) {
            if l.listening && !l.pending.is_empty() {
                mask |= EPOLLIN;
            }
            return mask;
        }
    }

    let (sock_type, shut_rd, shut_wr, peer_shut_wr, last_err) = {
        let meta = UNIX_SOCKET_META.lock();
        meta.iter()
            .find(|m| m.blob_id == blob_id)
            .map(|m| {
                (
                    m.sock_type,
                    m.shutdown_rd,
                    m.shutdown_wr,
                    m.peer_shutdown_wr,
                    m.last_error,
                )
            })
            .unwrap_or((1, false, false, false, 0))
    };

    if last_err != 0 {
        mask |= EPOLLERR;
    }

    if sock_type == 2 {
        // SOCK_DGRAM
        if dgram_queue_len(blob_id) > 0 || shut_rd {
            mask |= EPOLLIN;
        }
        if !shut_wr {
            let peer = socket_peer_blob(blob_id).unwrap_or(BlobId([0u8; 32]));
            if peer == BlobId([0u8; 32]) || dgram_queue_len(peer) < MAX_DGRAM_QUEUE_PACKETS {
                mask |= EPOLLOUT;
            }
        }
        if shut_rd && shut_wr {
            mask |= EPOLLHUP;
        }
        return mask;
    }

    // SOCK_STREAM
    let peer = socket_peer_blob(blob_id).unwrap_or(BlobId([0u8; 32]));
    let peer_closed = peer != BlobId([0u8; 32]) && is_socket_closed(peer);
    let has_data = socket_payload_len(blob_id) != 0;
    let has_ancillary = SOCKET_ANCILLARY_FDS
        .lock()
        .iter()
        .any(|(b, _, _, _)| *b == blob_id);

    if has_data || has_ancillary || peer_closed || shut_rd || peer_shut_wr {
        mask |= EPOLLIN;
    }
    if peer_closed || peer_shut_wr {
        mask |= EPOLLRDHUP;
    }
    if peer_closed || (shut_rd && shut_wr) {
        mask |= EPOLLHUP;
    }
    if !shut_wr && peer != BlobId([0u8; 32]) && !peer_closed {
        if socket_payload_len(peer) < SOCKET_MAX_CAPACITY {
            mask |= EPOLLOUT;
        }
    }
    mask
}

#[inline]
pub fn socket_is_readable(blob_id: BlobId) -> bool {
    (socket_readiness_mask(blob_id) & EPOLLIN) != 0
}

fn is_abstract_unix_addr(addr: &[u8]) -> bool {
    !addr.is_empty() && addr[0] == 0
}

/// S3: Confinement of Linux abstract `AF_UNIX` namespace (`sun_path[0] == 0`):
/// - Denied (`-EACCES`) by default for processes restricted via `SYS_EXO_PLEDGE`
///   without explicit `pledge_flags::UNIX_ABSTRACT` opt-in (`NO_UNIX_ABSTRACT`).
/// - Partitioned by `(pid_ns, net_ns)` so confined namespaces cannot contact
///   abstract endpoints outside their own domain.
fn check_and_scope_abstract_unix_addr(
    pid: u32,
    normalized: &mut Vec<u8>,
    syscall_nr: u32,
) -> Result<(), FsBridgeError> {
    if !is_abstract_unix_addr(normalized) {
        return Ok(());
    }
    let restrictions = crate::security::zero_trust::process_restrictions(pid);
    if (restrictions & crate::security::zero_trust::restriction_flags::NO_UNIX_ABSTRACT) != 0 {
        crate::security::shield_feed::push_event(
            pid,
            crate::security::shield_feed::event_type::SYSCALL,
            crate::security::shield_feed::severity::MEDIUM,
            syscall_nr,
            restrictions,
            0xAB57_8AC7,
        );
        return Err(FsBridgeError::PermDenied);
    }
    if let Some(pcb) =
        crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(crate::process::core::pid::Pid(pid))
    {
        if pcb.pid_ns != 0 || pcb.net_ns != 0 {
            let mut scoped = Vec::new();
            scoped
                .try_reserve_exact(normalized.len() + 9)
                .map_err(|_| FsBridgeError::NoMemory)?;
            scoped.push(0);
            scoped.extend_from_slice(&pcb.pid_ns.to_le_bytes());
            scoped.extend_from_slice(&pcb.net_ns.to_le_bytes());
            scoped.extend_from_slice(&normalized[1..]);
            *normalized = scoped;
        }
    }
    Ok(())
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
    if path_slice.is_empty() {
        return Err(FsBridgeError::Invalid);
    }
    // Linux abstract namespace: leading NUL followed by 1+ arbitrary name bytes.
    if path_slice[0] == 0 {
        if path_slice.len() < 2 {
            return Err(FsBridgeError::Invalid);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(path_slice.len())
            .map_err(|_| FsBridgeError::NoMemory)?;
        out.extend_from_slice(path_slice);
        return Ok(out);
    }
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
    register_unix_socket_meta(blob_id, clean_ty, pid);
    let obj_flags = open_flags::O_RDWR | ((ty as u32) & O_NONBLOCK);
    let obj_fd = OBJECT_TABLE
        .open(blob_id, obj_flags, 0, 0, pid as u64)
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
    let mut normalized = parse_sockaddr_un_path(addr_ptr, addr_len)?;
    check_and_scope_abstract_unix_addr(pid, &mut normalized, crate::syscall::numbers::SYS_BIND as u32)?;
    let is_abstract = is_abstract_unix_addr(&normalized);
    let is_dgram = socket_is_dgram(entry.blob_id);

    // Reject duplicate bind on an already-bound socket.
    {
        let meta = UNIX_SOCKET_META.lock();
        if let Some(m) = meta.iter().find(|m| m.blob_id == entry.blob_id) {
            if !m.bound_path.is_empty() {
                return Err(FsBridgeError::Invalid);
            }
        }
    }

    if is_abstract {
        if UNIX_LISTENERS.lock().iter().any(|l| l.path == normalized)
            || UNIX_DGRAM_ENDPOINTS
                .lock()
                .iter()
                .any(|(p, _)| *p == normalized)
        {
            return Err(FsBridgeError::AddrInUse);
        }
    } else {
        if path_entry(&normalized).is_ok() {
            return Err(FsBridgeError::AddrInUse);
        }
        let (parent_path, leaf) = split_parent_and_leaf(&normalized)?;
        ensure_directory_exists(&parent_path)?;
        let file_blob = allocate_fresh_file_blob(&normalized)?;
        BLOB_CACHE
            .insert(file_blob, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&file_blob);
        upsert_mode(file_blob, 0o140000 | 0o660);
        upsert_parent_entry(&parent_path, &leaf, file_blob, PATH_INDEX_KIND_FILE)?;
    }

    {
        let mut meta = UNIX_SOCKET_META.lock();
        if let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) {
            m.bound_path = normalized.clone();
        }
    }

    if is_dgram {
        let mut eps = UNIX_DGRAM_ENDPOINTS.lock();
        if eps.iter().any(|(p, _)| *p == normalized) {
            return Err(FsBridgeError::AddrInUse);
        }
        eps.try_reserve(1).map_err(|_| FsBridgeError::NoMemory)?;
        eps.push((normalized, entry.blob_id));
    } else {
        let mut listeners = UNIX_LISTENERS.lock();
        if let Some(existing) = listeners.iter_mut().find(|l| l.path == normalized) {
            existing.listener_blob = entry.blob_id;
            existing.pending.clear();
            existing.listening = false;
            existing.backlog = 0;
        } else {
            listeners
                .try_reserve(1)
                .map_err(|_| FsBridgeError::NoMemory)?;
            listeners.push(UnixListenerEntry {
                path: normalized,
                listener_blob: entry.blob_id,
                pending: Vec::new(),
                listening: false,
                backlog: 0,
            });
        }
    }
    Ok(0)
}

/// `listen(fd, backlog)`.
pub fn fs_unix_listen(fd: u32, backlog: i32, pid: u32) -> Result<i64, FsBridgeError> {
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    if socket_is_dgram(entry.blob_id) {
        return Err(FsBridgeError::OpNotSupp);
    }
    let mut listeners = UNIX_LISTENERS.lock();
    let Some(listener) = listeners
        .iter_mut()
        .find(|l| l.listener_blob == entry.blob_id)
    else {
        return Err(FsBridgeError::Invalid);
    };
    listener.listening = true;
    listener.backlog = (backlog.max(1) as usize).min(128);
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
    let mut normalized = parse_sockaddr_un_path(addr_ptr, addr_len)?;
    check_and_scope_abstract_unix_addr(pid, &mut normalized, crate::syscall::numbers::SYS_BIND as u32)?;
    let is_abstract = is_abstract_unix_addr(&normalized);

    if socket_is_dgram(entry.blob_id) {
        let target_blob = {
            let eps = UNIX_DGRAM_ENDPOINTS.lock();
            eps.iter()
                .find(|(p, _)| *p == normalized)
                .map(|(_, b)| *b)
        };
        let Some(target_blob) = target_blob else {
            return Err(if is_abstract || path_entry(&normalized).is_ok() {
                FsBridgeError::ConnRefused
            } else {
                FsBridgeError::NotFound
            });
        };
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        BLOB_CACHE
            .insert(entry.blob_id, socket_blob_with_peer(target_blob))
            .map_err(exofs_to_bridge_error)?;
        drop(_guard);
        let mut meta = UNIX_SOCKET_META.lock();
        if let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) {
            m.peer_path = normalized;
            let (uid, gid) = process_uid_gid(pid);
            m.cred_pid = pid;
            m.cred_uid = uid;
            m.cred_gid = gid;
        }
        return Ok(0);
    }

    let server_blob = next_pseudo_blob(PSEUDO_SOCKET_TAG);

    let _guard = SOCKET_PAYLOAD_LOCK.lock();
    if is_socket_closed(entry.blob_id) {
        return Err(FsBridgeError::Invalid);
    }
    let cur_data = snapshot_blob(&entry.blob_id)?;
    let cur_peer = parse_socket_peer_bytes(&cur_data)?;
    if cur_peer != BlobId([0u8; 32]) {
        return Err(FsBridgeError::IsConn);
    }

    let mut listeners = UNIX_LISTENERS.lock();
    if listeners.iter().any(|l| l.listener_blob == entry.blob_id) {
        return Err(FsBridgeError::Invalid);
    }
    let Some(listener) = listeners.iter_mut().find(|l| l.path == normalized) else {
        drop(listeners);
        drop(_guard);
        return Err(if is_abstract || path_entry(&normalized).is_ok() {
            FsBridgeError::ConnRefused
        } else {
            FsBridgeError::NotFound
        });
    };
    if !listener.listening {
        return Err(FsBridgeError::ConnRefused);
    }
    if listener.pending.len() >= listener.backlog {
        return Err(FsBridgeError::WouldBlock);
    }
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
    let listener_blob = listener.listener_blob;
    listener.pending.push(server_blob);
    drop(listeners);
    drop(_guard);

    // Record socket metadata for accepted server endpoint and client credentials at connect time.
    let (client_uid, client_gid) = process_uid_gid(pid);
    let (client_bound_path, server_cred) = {
        let mut meta = UNIX_SOCKET_META.lock();
        let l_cred = meta
            .iter()
            .find(|m| m.blob_id == listener_blob)
            .map(|m| (m.cred_pid, m.cred_uid, m.cred_gid, m.sock_type))
            .unwrap_or((pid, client_uid, client_gid, 1));
        let mut c_bound = Vec::new();
        if let Some(cm) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) {
            cm.peer_path = normalized.clone();
            cm.cred_pid = pid;
            cm.cred_uid = client_uid;
            cm.cred_gid = client_gid;
            c_bound = cm.bound_path.clone();
        }
        (c_bound, l_cred)
    };
    register_unix_socket_meta(server_blob, server_cred.3, server_cred.0);
    {
        let mut meta = UNIX_SOCKET_META.lock();
        if let Some(sm) = meta.iter_mut().find(|m| m.blob_id == server_blob) {
            sm.cred_pid = server_cred.0;
            sm.cred_uid = server_cred.1;
            sm.cred_gid = server_cred.2;
            sm.bound_path = normalized;
            sm.peer_path = client_bound_path;
        }
    }
    notify_io_event(Some(listener_blob));
    Ok(0)
}

fn write_sockaddr_un(addr_ptr: u64, addr_len_ptr: u64, path: &[u8]) -> Result<(), FsBridgeError> {
    if addr_len_ptr == 0 {
        return if addr_ptr == 0 {
            Ok(())
        } else {
            Err(FsBridgeError::Fault)
        };
    }
    let user_cap = read_user_typed::<u32>(addr_len_ptr).map_err(|_| FsBridgeError::Fault)? as usize;
    let mut raw = [0u8; 110];
    raw[0..2].copy_from_slice(&1u16.to_ne_bytes()); // AF_UNIX
    let full_len = if path.is_empty() {
        2usize
    } else if path[0] == 0 {
        // Abstract namespace: exact leading NUL + name bytes, no extra NUL terminator.
        let copy_n = path.len().min(108);
        raw[2..2 + copy_n].copy_from_slice(&path[..copy_n]);
        2 + copy_n
    } else {
        let copy_n = path.len().min(107);
        raw[2..2 + copy_n].copy_from_slice(&path[..copy_n]);
        raw[2 + copy_n] = 0;
        2 + copy_n + 1
    };
    if addr_ptr != 0 && user_cap > 0 {
        let to_copy = full_len.min(user_cap);
        copy_to_user(addr_ptr as *mut u8, raw.as_ptr(), to_copy)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    write_user_typed::<u32>(addr_len_ptr, full_len as u32).map_err(|_| FsBridgeError::Fault)?;
    Ok(())
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
    if socket_is_dgram(entry.blob_id) {
        return Err(FsBridgeError::OpNotSupp);
    }
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0;
    let server_blob = loop {
        let gen = io_wait_gen();
        {
            let mut listeners = UNIX_LISTENERS.lock();
            let Some(listener) = listeners
                .iter_mut()
                .find(|l| l.listener_blob == entry.blob_id)
            else {
                return Err(FsBridgeError::Invalid);
            };
            if !listener.listening {
                return Err(FsBridgeError::Invalid);
            }
            if !listener.pending.is_empty() {
                break listener.pending.remove(0);
            }
        }
        if nonblock {
            return Err(FsBridgeError::WouldBlock);
        }
        let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
    };
    if addr_ptr != 0 && addr_len_ptr != 0 {
        let peer_path = UNIX_SOCKET_META
            .lock()
            .iter()
            .find(|m| m.blob_id == server_blob)
            .map(|m| m.peer_path.clone())
            .unwrap_or_default();
        write_sockaddr_un(addr_ptr, addr_len_ptr, &peer_path)?;
    }
    let s_fd = OBJECT_TABLE
        .open(server_blob, open_flags::O_RDWR, 0, 0, pid as u64)
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

/// `shutdown(fd, how)` on `AF_UNIX` socket.
pub fn fs_unix_shutdown(fd: u32, how: i32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if !(0..=2).contains(&how) {
        return Err(FsBridgeError::Invalid);
    }
    let resolved = resolve_fd(pid, fd)?;
    let entry = OBJECT_TABLE
        .get(resolved.handle)
        .map_err(exofs_to_bridge_error)?;
    if !is_pseudo_blob(&entry.blob_id, PSEUDO_SOCKET_TAG) {
        return Err(FsBridgeError::Invalid);
    }
    let peer = socket_peer_blob(entry.blob_id)?;
    if peer == BlobId([0u8; 32]) && !socket_is_dgram(entry.blob_id) {
        return Err(FsBridgeError::NotConnected);
    }
    let shut_rd = how == 0 || how == 2;
    let shut_wr = how == 1 || how == 2;
    {
        let mut meta = UNIX_SOCKET_META.lock();
        if let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) {
            if shut_rd {
                m.shutdown_rd = true;
            }
            if shut_wr {
                m.shutdown_wr = true;
            }
        }
        if shut_wr && peer != BlobId([0u8; 32]) {
            if let Some(pm) = meta.iter_mut().find(|m| m.blob_id == peer) {
                pm.peer_shutdown_wr = true;
            }
        }
    }
    notify_io_event(Some(entry.blob_id));
    if peer != BlobId([0u8; 32]) {
        notify_io_event(Some(peer));
    }
    Ok(0)
}

/// `getsockname(fd, addr, addrlen)` on `AF_UNIX` socket.
pub fn fs_unix_getsockname(
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
    let path = UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == entry.blob_id)
        .map(|m| m.bound_path.clone())
        .unwrap_or_default();
    write_sockaddr_un(addr_ptr, addr_len_ptr, &path)?;
    Ok(0)
}

/// `getpeername(fd, addr, addrlen)` on `AF_UNIX` socket.
pub fn fs_unix_getpeername(
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
    let peer = socket_peer_blob(entry.blob_id)?;
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    let path = {
        let meta = UNIX_SOCKET_META.lock();
        meta.iter()
            .find(|m| m.blob_id == peer)
            .map(|m| m.bound_path.clone())
            .or_else(|| {
                meta.iter()
                    .find(|m| m.blob_id == entry.blob_id)
                    .map(|m| m.peer_path.clone())
            })
            .unwrap_or_default()
    };
    write_sockaddr_un(addr_ptr, addr_len_ptr, &path)?;
    Ok(0)
}

/// `setsockopt(fd, level, optname, optval, optlen)` on `AF_UNIX` socket.
pub fn fs_unix_setsockopt(
    fd: u32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen: u32,
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
    if optlen != 0 && optval == 0 {
        return Err(FsBridgeError::Fault);
    }
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_PASSCRED: i32 = 16;
    const SO_RCVTIMEO: i32 = 20;
    const SO_SNDTIMEO: i32 = 21;
    if level != SOL_SOCKET {
        return Err(FsBridgeError::NoProtoOpt);
    }
    let mut meta = UNIX_SOCKET_META.lock();
    let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) else {
        return Err(FsBridgeError::BadFd);
    };
    match optname {
        SO_REUSEADDR | SO_PASSCRED | SO_SNDBUF | SO_RCVBUF => {
            if optlen < 4 {
                return Err(FsBridgeError::Invalid);
            }
            let val = read_user_typed::<i32>(optval).map_err(|_| FsBridgeError::Fault)?;
            match optname {
                SO_REUSEADDR => m.reuseaddr = val != 0,
                SO_PASSCRED => m.passcred = val != 0,
                SO_SNDBUF => m.sndbuf = (val.max(4096) as u32).min(SOCKET_MAX_CAPACITY as u32),
                SO_RCVBUF => m.rcvbuf = (val.max(4096) as u32).min(SOCKET_MAX_CAPACITY as u32),
                _ => {}
            }
            Ok(0)
        }
        SO_RCVTIMEO | SO_SNDTIMEO => {
            if optlen < 16 {
                return Err(FsBridgeError::Invalid);
            }
            let _ts = read_user_typed::<LinuxTimeval>(optval).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::NoProtoOpt),
    }
}

/// `getsockopt(fd, level, optname, optval, optlen*)` on `AF_UNIX` socket.
pub fn fs_unix_getsockopt(
    fd: u32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen_ptr: u64,
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
    if optval == 0 || optlen_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const SO_SNDBUF: i32 = 7;
    const SO_RCVBUF: i32 = 8;
    const SO_PASSCRED: i32 = 16;
    const SO_PEERCRED: i32 = 17;
    if level != SOL_SOCKET {
        return Err(FsBridgeError::NoProtoOpt);
    }
    let user_len = read_user_typed::<u32>(optlen_ptr).map_err(|_| FsBridgeError::Fault)? as usize;
    match optname {
        SO_PEERCRED => {
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct LinuxUcred {
                pid: i32,
                uid: u32,
                gid: u32,
            }
            if user_len < size_of::<LinuxUcred>() {
                return Err(FsBridgeError::Invalid);
            }
            let peer = socket_peer_blob(entry.blob_id)?;
            let cred = {
                let meta = UNIX_SOCKET_META.lock();
                if peer != BlobId([0u8; 32]) {
                    meta.iter()
                        .find(|m| m.blob_id == peer)
                        .map(|m| LinuxUcred {
                            pid: m.cred_pid as i32,
                            uid: m.cred_uid,
                            gid: m.cred_gid,
                        })
                        .unwrap_or(LinuxUcred {
                            pid: pid as i32,
                            uid: 0,
                            gid: 0,
                        })
                } else {
                    LinuxUcred {
                        pid: 0,
                        uid: u32::MAX,
                        gid: u32::MAX,
                    }
                }
            };
            write_user_typed(optval, cred).map_err(|_| FsBridgeError::Fault)?;
            write_user_typed::<u32>(optlen_ptr, size_of::<LinuxUcred>() as u32)
                .map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        SO_TYPE | SO_ERROR | SO_REUSEADDR | SO_PASSCRED | SO_SNDBUF | SO_RCVBUF => {
            if user_len < 4 {
                return Err(FsBridgeError::Invalid);
            }
            let val: i32 = {
                let mut meta = UNIX_SOCKET_META.lock();
                let Some(m) = meta.iter_mut().find(|m| m.blob_id == entry.blob_id) else {
                    return Err(FsBridgeError::BadFd);
                };
                match optname {
                    SO_TYPE => m.sock_type,
                    SO_ERROR => {
                        let err = m.last_error;
                        m.last_error = 0;
                        err
                    }
                    SO_REUSEADDR => i32::from(m.reuseaddr),
                    SO_PASSCRED => i32::from(m.passcred),
                    SO_SNDBUF => m.sndbuf as i32,
                    SO_RCVBUF => m.rcvbuf as i32,
                    _ => 0,
                }
            };
            write_user_typed::<i32>(optval, val).map_err(|_| FsBridgeError::Fault)?;
            write_user_typed::<u32>(optlen_ptr, 4).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        _ => Err(FsBridgeError::NoProtoOpt),
    }
}

/// `sendto` on `AF_UNIX` socket (`SOCK_STREAM` and `SOCK_DGRAM`).
pub fn fs_unix_sendto(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
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
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    let nosignal = (flags & 0x4000) != 0;
    let input = if len == 0 {
        Vec::new()
    } else {
        read_user_bytes(buf_ptr, len)?
    };

    if socket_is_dgram(entry.blob_id) {
        if socket_is_write_shutdown(entry.blob_id) {
            if !nosignal {
                let _ = crate::process::signal::delivery::send_signal_to_pid(
                    crate::process::core::pid::Pid(pid),
                    crate::process::signal::default::Signal::SIGPIPE,
                );
            }
            return Err(FsBridgeError::BrokenPipe);
        }
        let target_blob = if addr_ptr != 0 {
            let mut normalized = parse_sockaddr_un_path(addr_ptr, addr_len)?;
            check_and_scope_abstract_unix_addr(
                pid,
                &mut normalized,
                crate::syscall::numbers::SYS_SENDTO as u32,
            )?;
            let is_abstract = is_abstract_unix_addr(&normalized);
            let found = UNIX_DGRAM_ENDPOINTS
                .lock()
                .iter()
                .find(|(p, _)| *p == normalized)
                .map(|(_, b)| *b);
            match found {
                Some(b) => b,
                None => {
                    return Err(if is_abstract || path_entry(&normalized).is_ok() {
                        FsBridgeError::ConnRefused
                    } else {
                        FsBridgeError::NotFound
                    });
                }
            }
        } else {
            let peer = socket_peer_blob(entry.blob_id)?;
            if peer == BlobId([0u8; 32]) {
                return Err(FsBridgeError::NotConnected);
            }
            peer
        };
        return dgram_send_to_blob(entry.blob_id, target_blob, &input, nonblock, nosignal, pid);
    }

    if addr_ptr != 0 {
        return Err(FsBridgeError::IsConn);
    }
    write_unix_socket_bytes(entry.blob_id, &input, flags, nonblock, pid)
}

/// `recvfrom` on `AF_UNIX` socket (`SOCK_STREAM` and `SOCK_DGRAM`).
pub fn fs_unix_recvfrom(
    fd: u32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
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
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    if socket_is_dgram(entry.blob_id) {
        return dgram_read_to_user(
            entry.blob_id,
            buf_ptr,
            len,
            consume,
            nonblock,
            flags,
            addr_ptr,
            addr_len_ptr,
        );
    }
    let n = socket_read_to_user(entry.blob_id, buf_ptr, len, consume, nonblock)?;
    if addr_ptr != 0 && addr_len_ptr != 0 {
        let peer_path = UNIX_SOCKET_META
            .lock()
            .iter()
            .find(|m| m.blob_id == entry.blob_id)
            .map(|m| m.peer_path.clone())
            .unwrap_or_default();
        write_sockaddr_un(addr_ptr, addr_len_ptr, &peer_path)?;
    }
    Ok(n)
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

fn rollback_staged_ancillary(staged: &[(BlobId, u32, u32, u32)]) {
    for &(_, h, _, _) in staged {
        let _ = close_object_handle_with_cleanup(h);
    }
}

/// S3 Zero-Trust policy enforcement for `SCM_RIGHTS` FD delegation:
/// - Rejects non-delegatable FDs (`FD_NO_DELEGATE`), TTY/hardware handles,
///   epoll/inotify control handles, and recursive `AF_UNIX` socket FDs (anti-cycle).
/// - Verifies `ExoCordon` (`ipc_policy`) and Zero-Trust `TrustLevel` between sender & receiver.
fn check_scm_rights_allowed(
    sender_pid: u32,
    peer_blob: BlobId,
    src_res: &ResolvedFd,
) -> Result<(), FsBridgeError> {
    let push_alert = |code: u64| {
        crate::security::shield_feed::push_event(
            sender_pid,
            crate::security::shield_feed::event_type::SYSCALL,
            crate::security::shield_feed::severity::MEDIUM,
            crate::syscall::numbers::SYS_SENDMSG as u32,
            src_res.handle as u64,
            code,
        );
    };

    // 1. Hardware / TTY / non-delegatable FDs are strictly forbidden in SCM_RIGHTS.
    if is_tty_handle(src_res.handle) || (src_res.flags & FD_NO_DELEGATE) != 0 {
        push_alert(0x5C01);
        return Err(FsBridgeError::OpNotPermitted);
    }
    let src_entry = OBJECT_TABLE
        .get(src_res.handle)
        .map_err(exofs_to_bridge_error)?;
    if (src_entry.flags & FD_NO_DELEGATE) != 0 {
        push_alert(0x5C02);
        return Err(FsBridgeError::OpNotPermitted);
    }

    // 2. Anti-cycle: forbid passing an AF_UNIX socket FD over AF_UNIX (eliminates Ring 0 cycles).
    if is_pseudo_blob(&src_entry.blob_id, PSEUDO_SOCKET_TAG) {
        push_alert(0x5C03);
        return Err(FsBridgeError::OpNotPermitted);
    }

    // 3. Forbid kernel control descriptors (epoll / inotify).
    if is_pseudo_blob(&src_entry.blob_id, PSEUDO_EPOLL_TAG)
        || is_pseudo_blob(&src_entry.blob_id, PSEUDO_INOTIFY_TAG)
    {
        push_alert(0x5C04);
        return Err(FsBridgeError::OpNotPermitted);
    }

    // 4. CapToken / Zero-Trust / ExoCordon check between sender and receiver.
    let receiver_pid = UNIX_SOCKET_META
        .lock()
        .iter()
        .find(|m| m.blob_id == peer_blob)
        .map(|m| m.cred_pid)
        .unwrap_or(sender_pid);

    let sender_trust = crate::security::zero_trust::trust_for_pid(sender_pid);
    let receiver_trust = crate::security::zero_trust::trust_for_pid(receiver_pid);
    if sender_trust == crate::security::zero_trust::TrustLevel::Untrusted
        || receiver_trust == crate::security::zero_trust::TrustLevel::Untrusted
    {
        push_alert(0x5C05);
        return Err(FsBridgeError::OpNotPermitted);
    }

    if sender_pid != receiver_pid {
        use crate::security::ipc_policy::{check_direct_ipc, service_class_of, IpcPolicyResult, ServiceClass};
        let sender_class = service_class_of(crate::process::core::pid::Pid(sender_pid));
        let receiver_class = service_class_of(crate::process::core::pid::Pid(receiver_pid));
        if sender_class != ServiceClass::Unknown
            && receiver_class != ServiceClass::Unknown
            && check_direct_ipc(
                crate::process::core::pid::Pid(sender_pid),
                crate::process::core::pid::Pid(receiver_pid),
            ) != IpcPolicyResult::Allowed
        {
            push_alert(0x5C06);
            return Err(FsBridgeError::OpNotPermitted);
        }
        let sender_restrictions = crate::security::zero_trust::process_restrictions(sender_pid);
        if (sender_restrictions
            & (crate::security::zero_trust::restriction_flags::PLEDGE_ACTIVE
                | crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC))
            != 0
            && matches!(
                receiver_class,
                ServiceClass::InitServer
                    | ServiceClass::CryptoServer
                    | ServiceClass::MemoryServer
                    | ServiceClass::DeviceServer
                    | ServiceClass::ExoShield
            )
        {
            push_alert(0x5C07);
            return Err(FsBridgeError::OpNotPermitted);
        }
    }

    Ok(())
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
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    let nosignal = (flags & 0x4000) != 0;
    if socket_is_write_shutdown(entry.blob_id) {
        if !nosignal {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        return Err(FsBridgeError::BrokenPipe);
    }
    let peer = socket_peer_blob(entry.blob_id)?;
    if peer == BlobId([0u8; 32]) {
        return Err(FsBridgeError::NotConnected);
    }
    if is_socket_closed(peer) {
        if !nosignal {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                crate::process::core::pid::Pid(pid),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
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

    // Validate and duplicate all SCM_RIGHTS ancillary FDs atomically before queuing (S3).
    let mut staged_fds: Vec<(BlobId, u32, u32, u32)> = Vec::new();
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
                if staged_fds.len().saturating_add(fd_count) > MAX_INFLIGHT_FDS_PER_SOCKET {
                    rollback_staged_ancillary(&staged_fds);
                    crate::security::shield_feed::push_event(
                        pid,
                        crate::security::shield_feed::event_type::SYSCALL,
                        crate::security::shield_feed::severity::MEDIUM,
                        crate::syscall::numbers::SYS_SENDMSG as u32,
                        fd_count as u64,
                        0x5C10,
                    );
                    return Err(FsBridgeError::TooManyFds);
                }
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
                    if let Err(err) = check_scm_rights_allowed(pid, peer, &src_res) {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(err);
                    }
                    let Ok(dup_handle) = OBJECT_TABLE.dup_checked_for_owner(src_res.handle, 0) else {
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::BadFd);
                    };
                    if staged_fds.try_reserve(1).is_err() {
                        let _ = close_object_handle_with_cleanup(dup_handle);
                        rollback_staged_ancillary(&staged_fds);
                        return Err(FsBridgeError::NoMemory);
                    }
                    staged_fds.push((peer, dup_handle, src_res.flags, pid));
                }
            }
            off += (hdr.cmsg_len + 7) & !7;
        }
    }

    loop {
        let gen = io_wait_gen();
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        if socket_is_write_shutdown(entry.blob_id) || is_socket_closed(peer) {
            drop(_guard);
            rollback_staged_ancillary(&staged_fds);
            if !nosignal {
                let _ = crate::process::signal::delivery::send_signal_to_pid(
                    crate::process::core::pid::Pid(pid),
                    crate::process::signal::default::Signal::SIGPIPE,
                );
            }
            return Err(FsBridgeError::BrokenPipe);
        }
        let result = if staged_fds.is_empty() {
            append_socket_payload_locked(peer, &payload)
        } else {
            let mut q = SOCKET_ANCILLARY_FDS.lock();
            let existing_sock = q.iter().filter(|(b, _, _, _)| *b == peer).count();
            let existing_pid = q.iter().filter(|(_, _, _, sp)| *sp == pid).count();
            if existing_sock.saturating_add(staged_fds.len()) > MAX_INFLIGHT_FDS_PER_SOCKET
                || existing_pid.saturating_add(staged_fds.len()) > MAX_INFLIGHT_FDS_PER_PID
            {
                crate::security::shield_feed::push_event(
                    pid,
                    crate::security::shield_feed::event_type::SYSCALL,
                    crate::security::shield_feed::severity::MEDIUM,
                    crate::syscall::numbers::SYS_SENDMSG as u32,
                    (existing_sock.max(existing_pid)) as u64,
                    0x5C11,
                );
                Err(FsBridgeError::TooManyFds)
            } else if q.try_reserve(staged_fds.len()).is_err() {
                Err(FsBridgeError::NoMemory)
            } else {
                match append_socket_payload_locked(peer, &payload) {
                    Ok(n) => {
                        q.extend(staged_fds.drain(..));
                        Ok(n)
                    }
                    Err(err) => Err(err),
                }
            }
        };
        drop(_guard);
        match result {
            Ok(written) => {
                notify_io_event(Some(peer));
                return Ok(written);
            }
            Err(FsBridgeError::WouldBlock) if !nonblock => {
                if let Err(err) = wait_for_io_event(gen, None, 1_000_000_000) {
                    rollback_staged_ancillary(&staged_fds);
                    return Err(err);
                }
            }
            Err(err) => {
                rollback_staged_ancillary(&staged_fds);
                return Err(err);
            }
        }
    }
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
    let nonblock = ((resolved.flags | entry.flags) & O_NONBLOCK) != 0 || (flags & 0x40) != 0;
    loop {
        let gen = io_wait_gen();
        let _guard = SOCKET_PAYLOAD_LOCK.lock();
        let data = read_socket_payload_locked(entry.blob_id, total_cap, false)?;
        let matching: Vec<(u32, u32)> = {
            let q = SOCKET_ANCILLARY_FDS.lock();
            q.iter()
                .filter(|(b, _, _, _)| *b == entry.blob_id)
                .map(|&(_, h, f, _)| (h, f))
                .collect()
        };
        if data.is_empty() && matching.is_empty() {
            let peer_closed =
                is_socket_peer_closed_locked(entry.blob_id) || socket_is_read_shutdown(entry.blob_id);
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
            let _ = wait_for_io_event(gen, None, 1_000_000_000)?;
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
        let mut delivered_fd_count = 0usize;
        // On MSG_PEEK (`!consume`), do NOT install duplicate FDs into the receiver's FD table
        // (avoids leaking FDs on repeated peeks; FDs remain queued for the consuming recvmsg).
        if consume && !matching.is_empty() {
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
                let dup_handle = match OBJECT_TABLE.dup_transferred(obj_handle, pid as u64) {
                    Ok(h) => h,
                    Err(err) => {
                        return Err(fail_recvmsg(
                            _guard,
                            pid,
                            &received_fds,
                            has_fd_table,
                            exofs_to_bridge_error(err),
                        ));
                    }
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
            delivered_fd_count = received_fds.len();

            if !received_fds.is_empty() {
                let cmsg_len = 16 + received_fds.len() * 4;
                let aligned_len = ((cmsg_len + 7) & !7).min(msg.msg_controllen as usize);
                let mut cmsg_bytes = Vec::new();
                if cmsg_bytes.try_reserve_exact(aligned_len).is_err() {
                    return Err(fail_recvmsg(
                        _guard,
                        pid,
                        &received_fds,
                        has_fd_table,
                        FsBridgeError::NoMemory,
                    ));
                }
                cmsg_bytes.resize(aligned_len, 0);
                cmsg_bytes[0..8].copy_from_slice(&cmsg_len.to_ne_bytes());
                cmsg_bytes[8..12].copy_from_slice(&1i32.to_ne_bytes());
                cmsg_bytes[12..16].copy_from_slice(&1i32.to_ne_bytes());
                for (idx, &(rfd, _)) in received_fds.iter().enumerate() {
                    let off = 16 + idx * 4;
                    cmsg_bytes[off..off + 4].copy_from_slice(&rfd.to_ne_bytes());
                }
                if copy_to_user(
                    msg.msg_control as *mut u8,
                    cmsg_bytes.as_ptr(),
                    aligned_len,
                )
                .is_err()
                {
                    return Err(fail_recvmsg(
                        _guard,
                        pid,
                        &received_fds,
                        has_fd_table,
                        FsBridgeError::Fault,
                    ));
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

        let peer = snapshot_blob(&entry.blob_id)
            .ok()
            .and_then(|d| parse_socket_peer_bytes(&d).ok())
            .unwrap_or(BlobId([0u8; 32]));

        if consume {
            if copied != 0 {
                let _ = read_socket_payload_locked(entry.blob_id, copied, true)?;
            }
            // Remove ONLY the `delivered_fd_count` FDs actually delivered to userspace.
            // Undelivered FDs on MSG_CTRUNC remain safely queued for subsequent recvmsg
            // (and are cleanly released by `on_socket_blob_closed` if the socket closes).
            if delivered_fd_count > 0 {
                let mut to_close = Vec::new();
                {
                    let mut q = SOCKET_ANCILLARY_FDS.lock();
                    let mut i = 0;
                    let mut removed = 0usize;
                    while i < q.len() && removed < delivered_fd_count {
                        if q[i].0 == entry.blob_id {
                            let (_, obj_handle, _, _) = q.remove(i);
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
            } else {
                drop(_guard);
            }
            if peer != BlobId([0u8; 32]) {
                notify_io_event(Some(peer));
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

