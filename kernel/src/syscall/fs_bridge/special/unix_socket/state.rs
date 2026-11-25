//! # syscall/fs_bridge/special/unix_socket/state.rs — `AF_UNIX` tables, readiness & abstract namespace

use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};

use super::dgram::{dgram_queue_len, MAX_DGRAM_QUEUE_PACKETS};

pub static SOCKET_PAYLOAD_LOCK: Mutex<()> = Mutex::new(());
pub static CLOSED_SOCKETS: Mutex<Vec<BlobId>> = Mutex::new(Vec::new());

pub const SOCKET_MAX_CAPACITY: usize = 131072;

#[inline]
pub(super) fn parse_socket_peer_bytes(data: &[u8]) -> Result<BlobId, FsBridgeError> {
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
pub(super) fn is_socket_peer_closed_locked(blob_id: BlobId) -> bool {
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

pub(super) fn process_uid_gid(pid: u32) -> (u32, u32) {
    crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| {
            let c = pcb.creds.lock();
            (c.euid, c.egid)
        })
        .unwrap_or((0, 0))
}

pub(super) fn register_unix_socket_meta(blob_id: BlobId, sock_type: i32, pid: u32) {
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

pub(super) fn remove_unix_socket_meta(blob_id: BlobId) {
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
pub(super) fn read_socket_payload_locked(
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

#[inline]
pub(super) fn append_socket_payload_locked(
    blob_id: BlobId,
    bytes: &[u8],
) -> Result<i64, FsBridgeError> {
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

pub(super) fn is_abstract_unix_addr(addr: &[u8]) -> bool {
    !addr.is_empty() && addr[0] == 0
}

/// S3: Confinement of Linux abstract `AF_UNIX` namespace (`sun_path[0] == 0`).
pub(super) fn check_and_scope_abstract_unix_addr(
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
    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
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

pub(super) fn parse_sockaddr_un_path(
    addr_ptr: u64,
    addr_len: u64,
) -> Result<Vec<u8>, FsBridgeError> {
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

pub(super) fn write_sockaddr_un(
    addr_ptr: u64,
    addr_len_ptr: u64,
    path: &[u8],
) -> Result<(), FsBridgeError> {
    if addr_len_ptr == 0 {
        return if addr_ptr == 0 {
            Ok(())
        } else {
            Err(FsBridgeError::Fault)
        };
    }
    let user_cap =
        read_user_typed::<u32>(addr_len_ptr).map_err(|_| FsBridgeError::Fault)? as usize;
    let mut raw = [0u8; 110];
    raw[0..2].copy_from_slice(&1u16.to_ne_bytes());
    let full_len = if path.is_empty() {
        2usize
    } else if path[0] == 0 {
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
