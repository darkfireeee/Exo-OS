//! # syscall/net_bridge/socket_map.rs — Unified `OBJECT_TABLE` / `pcb.files` socket mapping

use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::fs_bridge;
use exo_abi::errno::{EBADF, ENOMEM, ENOTSOCK};

use super::{
    dispatch_for, reply_u32, NET_HANDLE_TAG, NET_HANDLE_TAG_MASK, NET_OP_CLOSE, NET_OP_POLL,
    SOCK_STREAM,
};

#[derive(Clone, Copy)]
pub(super) struct NetSocketMeta {
    pub(super) blob_id: BlobId,
    pub(super) net_handle: u32,
    pub(super) owner_pid: u32,
    pub(super) sock_type: i32,
    pub(super) rcv_timeout_ns: u64,
    pub(super) snd_timeout_ns: u64,
}

pub(super) static NET_SOCKET_MAP: Mutex<Vec<NetSocketMeta>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
pub(super) struct ResolvedNetSocket {
    pub(super) blob_id: Option<BlobId>,
    pub(super) net_handle: u32,
    pub(super) owner_pid: u32,
    pub(super) nonblock: bool,
    pub(super) sock_type: i32,
    pub(super) rcv_timeout_ns: u64,
    pub(super) snd_timeout_ns: u64,
}

pub(super) fn install_net_socket_fd(
    net_handle: u32,
    owner_pid: u32,
    sock_type: i32,
    open_flags_raw: u32,
) -> Result<i64, i64> {
    if !fs_bridge::is_fs_ready() {
        return Ok(net_handle as i64);
    }
    let blob_id = fs_bridge::next_pseudo_blob(fs_bridge::PSEUDO_NET_SOCKET_TAG);
    if fs_bridge::ensure_blob_exists(blob_id).is_err() {
        let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
        return Err(ENOMEM);
    }
    let obj_flags = open_flags::O_RDWR | (open_flags_raw & fs_bridge::O_NONBLOCK);
    let obj_fd = match OBJECT_TABLE.open(blob_id, obj_flags, 0, 0, owner_pid as u64) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
    };
    {
        let mut map = NET_SOCKET_MAP.lock();
        if map.try_reserve(1).is_err() {
            drop(map);
            let _ = OBJECT_TABLE.close(obj_fd);
            let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
        map.push(NetSocketMeta {
            blob_id,
            net_handle,
            owner_pid,
            sock_type,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    if fs_bridge::process_has_fd_table(owner_pid) {
        let fd_flags = fs_bridge::fd_table_flags(open_flags_raw, open_flags::O_RDWR);
        if let Some(logical_fd) = fs_bridge::install_process_fd(owner_pid, obj_fd as u64, fd_flags)
        {
            return Ok(logical_fd as i64);
        }
        let _ = fs_bridge::close_object_handle_with_cleanup(obj_fd);
        return Err(ENOMEM);
    }
    Ok(obj_fd as i64)
}

pub(super) fn lookup_net_meta_by_blob(blob_id: BlobId) -> Option<NetSocketMeta> {
    NET_SOCKET_MAP
        .lock()
        .iter()
        .find(|m| m.blob_id == blob_id)
        .copied()
}

pub(super) fn resolve_net_fd(fd: i32, pid: u32) -> Result<ResolvedNetSocket, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if fs_bridge::is_fs_ready() {
        if let Ok(res) = fs_bridge::resolve_fd(pid, fd as u32) {
            if !fs_bridge::is_tty_handle(res.handle) {
                if let Ok(entry) = OBJECT_TABLE.get(res.handle) {
                    if fs_bridge::is_pseudo_blob(&entry.blob_id, fs_bridge::PSEUDO_NET_SOCKET_TAG) {
                        let meta = lookup_net_meta_by_blob(entry.blob_id).ok_or(EBADF)?;
                        let nonblock = ((res.flags | entry.flags) & fs_bridge::O_NONBLOCK) != 0;
                        return Ok(ResolvedNetSocket {
                            blob_id: Some(entry.blob_id),
                            net_handle: meta.net_handle,
                            owner_pid: meta.owner_pid,
                            nonblock,
                            sock_type: meta.sock_type,
                            rcv_timeout_ns: meta.rcv_timeout_ns,
                            snd_timeout_ns: meta.snd_timeout_ns,
                        });
                    }
                    return Err(ENOTSOCK);
                }
            }
            return Err(ENOTSOCK);
        }
    }
    if let Some(raw_handle) = socket_handle_from_raw(fd as u64) {
        return Ok(ResolvedNetSocket {
            blob_id: None,
            net_handle: raw_handle as u32,
            owner_pid: pid,
            nonblock: false,
            sock_type: SOCK_STREAM,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    Err(EBADF)
}

pub fn socket_handle_from_raw(raw: u64) -> Option<i32> {
    if raw > i32::MAX as u64 {
        return None;
    }
    let fd = raw as u32;
    (fd & NET_HANDLE_TAG_MASK == NET_HANDLE_TAG).then_some(fd as i32)
}

/// Called by `fs_bridge::cleanup_closed_blob` when the final `OBJECT_TABLE` reference
/// to a `PSEUDO_NET_SOCKET_TAG` blob is closed.
pub fn on_net_socket_blob_closed(blob_id: BlobId) {
    let removed = {
        let mut map = NET_SOCKET_MAP.lock();
        map.iter()
            .position(|m| m.blob_id == blob_id)
            .map(|pos| map.swap_remove(pos))
    };
    if let Some(meta) = removed {
        let _ = dispatch_for(meta.owner_pid, NET_OP_CLOSE, meta.net_handle, 0, 0, 0, 0);
    }
    fs_bridge::notify_io_event(Some(blob_id));
}

/// Returns real Linux `poll`/`epoll` readiness bitmask from `network_server`.
pub fn net_poll_mask_for_blob(blob_id: BlobId) -> u32 {
    let Some(meta) = lookup_net_meta_by_blob(blob_id) else {
        return fs_bridge::EPOLLHUP;
    };
    match dispatch_for(meta.owner_pid, NET_OP_POLL, meta.net_handle, 0, 0, 0, 0) {
        Ok(reply) => reply_u32(&reply, 0),
        Err(_) => fs_bridge::EPOLLERR | fs_bridge::EPOLLHUP,
    }
}

/// Returns available RX byte count for `ioctl(FIONREAD)`.
pub fn net_available_bytes_for_blob(blob_id: BlobId) -> usize {
    let Some(meta) = lookup_net_meta_by_blob(blob_id) else {
        return 0;
    };
    match dispatch_for(meta.owner_pid, NET_OP_POLL, meta.net_handle, 0, 0, 0, 0) {
        Ok(reply) => reply_u32(&reply, 4) as usize,
        Err(_) => 0,
    }
}
