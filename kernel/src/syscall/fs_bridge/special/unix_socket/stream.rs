//! # syscall/fs_bridge/special/unix_socket/stream.rs — `AF_UNIX` connection lifecycle & stream I/O

use alloc::vec::Vec;

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{copy_to_user, write_user_typed};

use super::dgram::{dgram_read_to_user, dgram_send_to_blob};
use super::state::{
    check_and_scope_abstract_unix_addr, is_abstract_unix_addr, parse_sockaddr_un_path,
    parse_socket_peer_bytes, process_uid_gid, register_unix_socket_meta, write_sockaddr_un,
};

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
    check_and_scope_abstract_unix_addr(
        pid,
        &mut normalized,
        crate::syscall::numbers::SYS_BIND as u32,
    )?;
    let is_abstract = is_abstract_unix_addr(&normalized);
    let is_dgram = socket_is_dgram(entry.blob_id);

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
    check_and_scope_abstract_unix_addr(
        pid,
        &mut normalized,
        crate::syscall::numbers::SYS_BIND as u32,
    )?;
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
