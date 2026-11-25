//! # syscall/net_bridge/ops_io.rs — BSD socket data path (`sendto`, `recvfrom`, `sendmsg`, `recvmsg`, `sockopt`)

use crate::fs::exofs::core::BlobId;
use crate::process::core::pid::Pid;
use crate::syscall::errno::EMSGSIZE;
use crate::syscall::fs_bridge;
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};
use exo_abi::errno::{EAGAIN, EBADF, EFAULT, EINVAL, EPIPE};

use super::data_window::{
    copy_caller_user_to_net_window, copy_net_window_to_caller_user, ensure_net_data_window,
    lock_net_data_window,
};
use super::ops_connect::{read_sockaddr_in, write_sockaddr_in};
use super::socket_map::{
    lookup_net_meta_by_blob, resolve_net_fd, ResolvedNetSocket, NET_SOCKET_MAP,
};
use super::{
    call_network_raw, check_net_permission, current_pid, dispatch_for, make_msg_for, reply_u16,
    reply_u32, wait_net_slice, NetReply, IOV_MAX, MSG_DONTWAIT, MSG_NOSIGNAL, MSG_PEEK,
    NET_FLAG_USE_DATA_WINDOW, NET_INLINE_DATA_MAX, NET_OP_GETSOCKOPT, NET_OP_RECVFROM,
    NET_OP_SENDTO, NET_OP_SETSOCKOPT, NET_REPLY_DATA_OFFSET, SOCKADDR_IN_LEN, SOCK_STREAM,
    SOL_SOCKET, SO_RCVTIMEO, SO_SNDTIMEO,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIovec {
    iov_base: u64,
    iov_len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxMsghdr {
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

const _: () = assert!(core::mem::size_of::<LinuxMsghdr>() == 56);

fn msghdr_total_iov_len(msg: &LinuxMsghdr) -> Result<usize, i64> {
    if msg.msg_iovlen > IOV_MAX {
        return Err(EINVAL);
    }
    if msg.msg_iovlen != 0 && msg.msg_iov == 0 {
        return Err(EFAULT);
    }

    let mut total = 0usize;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(EFAULT)?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| EFAULT)?;
        if iov.iov_len > isize::MAX as u64 {
            return Err(EINVAL);
        }
        if iov.iov_len != 0 && iov.iov_base == 0 {
            return Err(EFAULT);
        }
        total = total.checked_add(iov.iov_len as usize).ok_or(EINVAL)?;
        idx += 1;
    }
    Ok(total)
}

pub fn net_sendto_blob(
    blob_id: BlobId,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    let meta = lookup_net_meta_by_blob(blob_id).ok_or(EBADF)?;
    let sock = ResolvedNetSocket {
        blob_id: Some(blob_id),
        net_handle: meta.net_handle,
        owner_pid: meta.owner_pid,
        nonblock: (flags & MSG_DONTWAIT) != 0,
        sock_type: meta.sock_type,
        rcv_timeout_ns: meta.rcv_timeout_ns,
        snd_timeout_ns: meta.snd_timeout_ns,
    };
    net_sendto_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len)
}

pub fn net_recvfrom_blob(
    blob_id: BlobId,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    let meta = lookup_net_meta_by_blob(blob_id).ok_or(EBADF)?;
    let sock = ResolvedNetSocket {
        blob_id: Some(blob_id),
        net_handle: meta.net_handle,
        owner_pid: meta.owner_pid,
        nonblock: (flags & MSG_DONTWAIT) != 0,
        sock_type: meta.sock_type,
        rcv_timeout_ns: meta.rcv_timeout_ns,
        snd_timeout_ns: meta.snd_timeout_ns,
    };
    net_recvfrom_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len_ptr)
}

pub fn net_sendto(
    fd: i32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_sendto(fd as u32, buf_ptr, len, flags, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SENDTO)?;
    let sock = resolve_net_fd(fd, pid)?;
    net_sendto_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len)
}

fn net_sendto_resolved(
    sock: ResolvedNetSocket,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(EFAULT);
    }
    let (addr, port) = if addr_ptr != 0 {
        read_sockaddr_in(addr_ptr, addr_len)?
    } else {
        (0, 0)
    };
    let nonblock = sock.nonblock || (flags & MSG_DONTWAIT) != 0;
    let is_dgram = sock.sock_type != SOCK_STREAM;
    let deadline = if sock.snd_timeout_ns != 0 {
        Some(
            crate::scheduler::timer::clock::monotonic_ns().saturating_add(sock.snd_timeout_ns),
        )
    } else {
        None
    };

    let window_opt = ensure_net_data_window();
    let max_chunk = window_opt
        .as_ref()
        .map(|w| w.len)
        .unwrap_or(NET_INLINE_DATA_MAX);
    if is_dgram && len > max_chunk {
        return Err(EMSGSIZE);
    }

    let mut total_sent: usize = 0;
    while total_sent < len {
        let chunk_len = (len - total_sent).min(max_chunk);
        let curr_buf = match buf_ptr.checked_add(total_sent as u64) {
            Some(p) => p,
            None => {
                return if total_sent > 0 {
                    Ok(total_sent as i64)
                } else {
                    Err(EFAULT)
                };
            }
        };

        let reply_status = if let Some(ref win) = window_opt {
            if chunk_len > NET_INLINE_DATA_MAX || is_dgram {
                let _lock = match lock_net_data_window() {
                    Ok(g) => g,
                    Err(e) => {
                        return if total_sent > 0 {
                            Ok(total_sent as i64)
                        } else {
                            Err(e)
                        };
                    }
                };
                if let Err(e) = copy_caller_user_to_net_window(win, curr_buf, chunk_len) {
                    return if total_sent > 0 {
                        Ok(total_sent as i64)
                    } else {
                        Err(e)
                    };
                }
                let msg = make_msg_for(
                    sock.owner_pid,
                    NET_OP_SENDTO,
                    sock.net_handle,
                    chunk_len as u64,
                    addr as u64,
                    port as u32,
                    (flags & !NET_FLAG_USE_DATA_WINDOW) | NET_FLAG_USE_DATA_WINDOW,
                );
                let mut reply_raw = [0u8; core::mem::size_of::<NetReply>()];
                match call_network_raw(&msg, &[], &mut reply_raw) {
                    Ok(_) => {
                        let r = unsafe {
                            core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply)
                        };
                        r.status
                    }
                    Err(e) => {
                        return if total_sent > 0 {
                            Ok(total_sent as i64)
                        } else {
                            Err(e)
                        };
                    }
                }
            } else {
                send_inline_chunk(&sock, curr_buf, chunk_len, addr, port, flags)?
            }
        } else {
            send_inline_chunk(&sock, curr_buf, chunk_len, addr, port, flags)?
        };

        if reply_status == EAGAIN {
            if total_sent > 0 {
                return Ok(total_sent as i64);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if !wait_net_slice(deadline)? {
                return Err(EAGAIN);
            }
            continue;
        }
        if reply_status == EPIPE && (flags & MSG_NOSIGNAL) == 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                Pid(current_pid()),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        if reply_status < 0 {
            return if total_sent > 0 {
                Ok(total_sent as i64)
            } else {
                Err(reply_status)
            };
        }
        let sent_this_chunk = (reply_status as usize).min(chunk_len);
        total_sent += sent_this_chunk;
        fs_bridge::notify_io_event(sock.blob_id);
        if is_dgram || sent_this_chunk < chunk_len {
            break;
        }
    }
    Ok(total_sent as i64)
}

fn send_inline_chunk(
    sock: &ResolvedNetSocket,
    curr_buf: u64,
    chunk_len: usize,
    addr: u32,
    port: u16,
    flags: u32,
) -> Result<i64, i64> {
    let mut data = [0u8; NET_INLINE_DATA_MAX];
    if copy_from_user(data.as_mut_ptr(), curr_buf as *const u8, chunk_len).is_err() {
        return Err(EFAULT);
    }
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_SENDTO,
        sock.net_handle,
        chunk_len as u64,
        addr as u64,
        port as u32,
        flags & !NET_FLAG_USE_DATA_WINDOW,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let _ = call_network_raw(&msg, &data[..chunk_len], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    Ok(reply.status)
}

pub fn net_recvfrom(
    fd: i32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_recvfrom(
            fd as u32,
            buf_ptr,
            len,
            flags,
            addr_ptr,
            addr_len_ptr,
            pid,
        )
        .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_RECVFROM)?;
    let sock = resolve_net_fd(fd, pid)?;
    net_recvfrom_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len_ptr)
}

fn net_recvfrom_resolved(
    sock: ResolvedNetSocket,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(EFAULT);
    }
    let nonblock = sock.nonblock || (flags & MSG_DONTWAIT) != 0;
    let is_dgram = sock.sock_type != SOCK_STREAM;
    let deadline = if sock.rcv_timeout_ns != 0 {
        Some(
            crate::scheduler::timer::clock::monotonic_ns().saturating_add(sock.rcv_timeout_ns),
        )
    } else {
        None
    };

    let window_opt = ensure_net_data_window();
    let max_chunk = window_opt
        .as_ref()
        .map(|w| w.len)
        .unwrap_or(NET_INLINE_DATA_MAX);

    let mut total_recv: usize = 0;
    let mut peer_addr_recorded = false;

    while total_recv < len {
        let chunk_len = (len - total_recv).min(max_chunk);
        let chunk_flags = if total_recv == 0 {
            flags & !NET_FLAG_USE_DATA_WINDOW
        } else {
            (flags & !NET_FLAG_USE_DATA_WINDOW) | MSG_DONTWAIT
        };
        let curr_buf = match buf_ptr.checked_add(total_recv as u64) {
            Some(p) => p,
            None => {
                return if total_recv > 0 {
                    Ok(total_recv as i64)
                } else {
                    Err(EFAULT)
                };
            }
        };

        let (reply, copied_len) = if let Some(ref win) = window_opt {
            if chunk_len > NET_INLINE_DATA_MAX || is_dgram {
                let _lock = match lock_net_data_window() {
                    Ok(g) => g,
                    Err(e) => {
                        return if total_recv > 0 {
                            Ok(total_recv as i64)
                        } else {
                            Err(e)
                        };
                    }
                };
                let msg = make_msg_for(
                    sock.owner_pid,
                    NET_OP_RECVFROM,
                    sock.net_handle,
                    chunk_len as u64,
                    0,
                    chunk_flags | NET_FLAG_USE_DATA_WINDOW,
                    0,
                );
                let mut reply_raw = [0u8; core::mem::size_of::<NetReply>()];
                if let Err(e) = call_network_raw(&msg, &[], &mut reply_raw) {
                    return if total_recv > 0 {
                        Ok(total_recv as i64)
                    } else {
                        Err(e)
                    };
                }
                let r =
                    unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
                if r.status > 0 {
                    let n = (r.status as usize).min(chunk_len);
                    if let Err(e) = copy_net_window_to_caller_user(win, curr_buf, n) {
                        return if total_recv > 0 {
                            Ok(total_recv as i64)
                        } else {
                            Err(e)
                        };
                    }
                    (r, n)
                } else {
                    (r, 0)
                }
            } else {
                recv_inline_chunk(&sock, curr_buf, chunk_len, chunk_flags, total_recv)?
            }
        } else {
            recv_inline_chunk(&sock, curr_buf, chunk_len, chunk_flags, total_recv)?
        };

        if reply.status == EAGAIN {
            if total_recv == 0 && !nonblock {
                if !wait_net_slice(deadline)? {
                    return Err(EAGAIN);
                }
                continue;
            }
            return if total_recv > 0 {
                Ok(total_recv as i64)
            } else {
                Err(EAGAIN)
            };
        }
        if reply.status < 0 {
            return if total_recv > 0 {
                Ok(total_recv as i64)
            } else {
                Err(reply.status)
            };
        }
        if !peer_addr_recorded {
            let peer_addr = reply_u32(&reply, 8);
            let peer_port = reply_u16(&reply, 12);
            if peer_addr != 0 || addr_ptr != 0 {
                write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port)?;
            }
            peer_addr_recorded = true;
        }
        total_recv += copied_len;
        if copied_len > 0 {
            fs_bridge::notify_io_event(sock.blob_id);
        }
        if is_dgram || copied_len == 0 || copied_len < chunk_len || (flags & MSG_PEEK) != 0 {
            break;
        }
    }
    Ok(total_recv as i64)
}

fn recv_inline_chunk(
    sock: &ResolvedNetSocket,
    curr_buf: u64,
    chunk_len: usize,
    chunk_flags: u32,
    total_recv: usize,
) -> Result<(NetReply, usize), i64> {
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_RECVFROM,
        sock.net_handle,
        chunk_len as u64,
        0,
        chunk_flags & !NET_FLAG_USE_DATA_WINDOW,
        0,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let n_raw = match call_network_raw(&msg, &[], &mut reply_raw) {
        Ok(n) => n,
        Err(e) => {
            return Err(if total_recv > 0 { EAGAIN } else { e });
        }
    };
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status <= 0 {
        return Ok((reply, 0));
    }
    let data_len = (reply.status as usize)
        .min(chunk_len)
        .min(n_raw.saturating_sub(NET_REPLY_DATA_OFFSET));
    if data_len != 0
        && copy_to_user(
            curr_buf as *mut u8,
            reply_raw[NET_REPLY_DATA_OFFSET..].as_ptr(),
            data_len,
        )
        .is_err()
    {
        return Err(EFAULT);
    }
    Ok((reply, data_len))
}

pub fn net_sendmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_sendmsg(fd as u32, msg_ptr, flags, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SENDMSG)?;
    let msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let _total_len = msghdr_total_iov_len(&msg)?;

    let mut total_sent: usize = 0;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(if total_sent > 0 {
                total_sent as i64
            } else {
                EFAULT
            })?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| {
            if total_sent > 0 {
                total_sent as i64
            } else {
                EFAULT
            }
        })?;
        let chunk_len = iov.iov_len as usize;
        if chunk_len != 0 {
            if iov.iov_base == 0 {
                return if total_sent > 0 {
                    Ok(total_sent as i64)
                } else {
                    Err(EFAULT)
                };
            }
            match net_sendto(
                fd,
                iov.iov_base,
                chunk_len,
                flags,
                msg.msg_name,
                msg.msg_namelen as u64,
            ) {
                Ok(n) => {
                    let sent = n.max(0) as usize;
                    total_sent += sent;
                    if sent < chunk_len {
                        break;
                    }
                }
                Err(err) => {
                    return if total_sent > 0 {
                        Ok(total_sent as i64)
                    } else {
                        Err(err)
                    };
                }
            }
        }
        idx += 1;
    }
    Ok(total_sent as i64)
}

pub fn net_recvmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_recvmsg(fd as u32, msg_ptr, flags, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_RECVMSG)?;
    let mut msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let _total_len = msghdr_total_iov_len(&msg)?;

    let mut total_recv: usize = 0;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(if total_recv > 0 {
                total_recv as i64
            } else {
                EFAULT
            })?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| {
            if total_recv > 0 {
                total_recv as i64
            } else {
                EFAULT
            }
        })?;
        let chunk_len = iov.iov_len as usize;
        if chunk_len != 0 {
            if iov.iov_base == 0 {
                return if total_recv > 0 {
                    Ok(total_recv as i64)
                } else {
                    Err(EFAULT)
                };
            }
            let effective_flags = if total_recv == 0 {
                flags
            } else {
                flags | MSG_DONTWAIT
            };
            let addr_out = if total_recv == 0 { msg.msg_name } else { 0 };
            match net_recvfrom(fd, iov.iov_base, chunk_len, effective_flags, addr_out, 0) {
                Ok(n) => {
                    let recvd = n.max(0) as usize;
                    if total_recv == 0 && msg.msg_name != 0 {
                        msg.msg_namelen = SOCKADDR_IN_LEN as u32;
                    }
                    total_recv += recvd;
                    if recvd < chunk_len || (flags & MSG_PEEK) != 0 {
                        break;
                    }
                }
                Err(err) => {
                    return if total_recv > 0 {
                        msg.msg_flags = 0;
                        let _ = write_user_typed::<LinuxMsghdr>(msg_ptr, msg);
                        Ok(total_recv as i64)
                    } else {
                        Err(err)
                    };
                }
            }
        }
        idx += 1;
    }
    msg.msg_flags = 0;
    write_user_typed::<LinuxMsghdr>(msg_ptr, msg).map_err(|_| EFAULT)?;
    Ok(total_recv as i64)
}

pub fn net_setsockopt(
    fd: i32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen: u32,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_setsockopt(fd as u32, level, optname, optval, optlen, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    if optlen != 0 && optval == 0 {
        return Err(EFAULT);
    }

    if level == SOL_SOCKET && (optname == SO_RCVTIMEO || optname == SO_SNDTIMEO) {
        if optlen < core::mem::size_of::<fs_bridge::LinuxTimeval>() as u32 {
            return Err(EINVAL);
        }
        let tv = read_user_typed::<fs_bridge::LinuxTimeval>(optval).map_err(|_| EFAULT)?;
        if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
            return Err(EINVAL);
        }
        let ns = (tv.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add((tv.tv_usec as u64).saturating_mul(1_000));
        if let Some(blob_id) = sock.blob_id {
            let mut map = NET_SOCKET_MAP.lock();
            if let Some(m) = map.iter_mut().find(|m| m.blob_id == blob_id) {
                if optname == SO_RCVTIMEO {
                    m.rcv_timeout_ns = ns;
                } else {
                    m.snd_timeout_ns = ns;
                }
            }
        }
        return Ok(0);
    }

    let copy_len = (optlen as usize).min(NET_INLINE_DATA_MAX);
    let mut inline = [0u8; NET_INLINE_DATA_MAX];
    if copy_len != 0 && copy_from_user(inline.as_mut_ptr(), optval as *const u8, copy_len).is_err()
    {
        return Err(EFAULT);
    }
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_SETSOCKOPT,
        sock.net_handle,
        level as u32 as u64,
        optname as u32 as u64,
        optlen,
        0,
    );
    let mut reply_buf = [0u8; core::mem::size_of::<NetReply>()];
    let n = call_network_raw(&msg, &inline[..copy_len], &mut reply_buf)?;
    if n < core::mem::size_of::<NetReply>() {
        return Err(EINVAL);
    }
    let reply = unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
    Ok(0)
}

pub fn net_getsockopt(
    fd: i32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen_ptr: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getsockopt(fd as u32, level, optname, optval, optlen_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;

    if level == SOL_SOCKET && (optname == SO_RCVTIMEO || optname == SO_SNDTIMEO) {
        if optval == 0 || optlen_ptr == 0 {
            return Err(EFAULT);
        }
        let user_cap = read_user_typed::<u32>(optlen_ptr).map_err(|_| EFAULT)? as usize;
        if user_cap < core::mem::size_of::<fs_bridge::LinuxTimeval>() {
            return Err(EINVAL);
        }
        let ns = if optname == SO_RCVTIMEO {
            sock.rcv_timeout_ns
        } else {
            sock.snd_timeout_ns
        };
        let tv = fs_bridge::LinuxTimeval {
            tv_sec: (ns / 1_000_000_000) as i64,
            tv_usec: ((ns % 1_000_000_000) / 1_000) as i64,
        };
        write_user_typed(optval, tv).map_err(|_| EFAULT)?;
        write_user_typed(
            optlen_ptr,
            core::mem::size_of::<fs_bridge::LinuxTimeval>() as u32,
        )
        .map_err(|_| EFAULT)?;
        return Ok(0);
    }

    let reply = dispatch_for(
        sock.owner_pid,
        NET_OP_GETSOCKOPT,
        sock.net_handle,
        level as u32 as u64,
        optname as u32 as u64,
        0,
        0,
    )?;
    let val_len = (reply_u32(&reply, 24) as usize).clamp(4, 8);
    if optval != 0 {
        let user_cap = if optlen_ptr != 0 {
            read_user_typed::<u32>(optlen_ptr).map_err(|_| EFAULT)? as usize
        } else {
            val_len
        };
        let to_copy = val_len.min(user_cap);
        if to_copy != 0
            && copy_to_user(optval as *mut u8, reply.payload[16..].as_ptr(), to_copy).is_err()
        {
            return Err(EFAULT);
        }
    }
    if optlen_ptr != 0 {
        let out_len = val_len as u32;
        if copy_to_user(
            optlen_ptr as *mut u8,
            &out_len as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    Ok(0)
}
