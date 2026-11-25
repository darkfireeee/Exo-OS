//! # syscall/fs_bridge/special/unix_socket/dgram.rs — `AF_UNIX` `SOCK_DGRAM` packet queues & `sendto`/`recvfrom`

use alloc::vec::Vec;

use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::OBJECT_TABLE;
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::copy_to_user;

use super::state::{
    check_and_scope_abstract_unix_addr, is_abstract_unix_addr, parse_sockaddr_un_path,
    write_sockaddr_un,
};

pub(super) const MAX_DGRAM_QUEUE_PACKETS: usize = 64;
pub(super) const MAX_DGRAM_PACKET_SIZE: usize = 65536;

#[inline]
pub(super) fn dgram_queue_len(blob_id: BlobId) -> usize {
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

pub(super) fn dgram_send_to_blob(
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

pub(super) fn dgram_read_to_user(
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
