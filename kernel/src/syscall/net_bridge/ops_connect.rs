//! # syscall/net_bridge/ops_connect.rs — BSD socket lifecycle & connection establishment

use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::fs_bridge;
use crate::syscall::validation::{copy_from_user, copy_to_user};
use exo_abi::errno::{
    EAGAIN, EBADF, EFAULT, EINPROGRESS, EINVAL, ENOMEM, ENOTSUP, ETIMEDOUT,
};

use super::socket_map::{
    install_net_socket_fd, resolve_net_fd, NetSocketMeta, NET_SOCKET_MAP,
};
use super::{
    check_net_permission, current_pid, dispatch_for, reply_u16, reply_u32, reply_u64,
    wait_net_slice, AF_INET, AF_UNIX, CONNECT_TIMEOUT_NS, NET_OP_ACCEPT, NET_OP_BIND,
    NET_OP_CLOSE, NET_OP_CONNECT, NET_OP_GETPEERNAME, NET_OP_GETSOCKNAME, NET_OP_LISTEN,
    NET_OP_OPEN, NET_OP_SHUTDOWN, NET_OP_SOCKETPAIR, SOCKADDR_IN_LEN, SOCK_STREAM,
};

pub(super) fn read_sockaddr_in(addr_ptr: u64, addr_len: u64) -> Result<(u32, u16), i64> {
    if addr_ptr == 0 || addr_len < SOCKADDR_IN_LEN as u64 {
        return Err(EINVAL);
    }
    let mut raw = [0u8; SOCKADDR_IN_LEN];
    if copy_from_user(raw.as_mut_ptr(), addr_ptr as *const u8, raw.len()).is_err() {
        return Err(EFAULT);
    }
    let family = u16::from_ne_bytes([raw[0], raw[1]]);
    if family != AF_INET {
        return Err(ENOTSUP);
    }
    let port = u16::from_be_bytes([raw[2], raw[3]]);
    let addr = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
    Ok((addr, port))
}

pub(super) fn write_sockaddr_in(
    addr_ptr: u64,
    addr_len_ptr: u64,
    addr: u32,
    port: u16,
) -> Result<(), i64> {
    if addr_ptr != 0 {
        let mut raw = [0u8; SOCKADDR_IN_LEN];
        raw[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
        raw[2..4].copy_from_slice(&port.to_be_bytes());
        raw[4..8].copy_from_slice(&addr.to_be_bytes());
        if copy_to_user(addr_ptr as *mut u8, raw.as_ptr(), raw.len()).is_err() {
            return Err(EFAULT);
        }
    }
    if addr_len_ptr != 0 {
        let len = SOCKADDR_IN_LEN as u32;
        if copy_to_user(
            addr_len_ptr as *mut u8,
            &len as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    Ok(())
}

pub fn net_socket(domain: i32, ty: i32, protocol: i32) -> Result<i64, i64> {
    if domain == AF_UNIX {
        return fs_bridge::fs_unix_socket(domain, ty, protocol, current_pid())
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SOCKET)?;
    let pid = current_pid();
    let clean_ty = ty & !((fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC) as i32);
    let open_flags_raw = (ty as u32) & (fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC);
    let reply = dispatch_for(
        pid,
        NET_OP_OPEN,
        0,
        domain as u32 as u64,
        clean_ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    let net_handle = reply_u64(&reply, 0) as u32;
    install_net_socket_fd(net_handle, pid, clean_ty, open_flags_raw)
}

pub fn net_close(fd: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    dispatch_for(current_pid(), NET_OP_CLOSE, fd as u32, 0, 0, 0, 0)?;
    Ok(0)
}

pub fn net_bind(fd: i32, addr_ptr: u64, addr_len: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_bind(fd as u32, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_BIND)?;
    let sock = resolve_net_fd(fd, pid)?;
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_BIND,
        sock.net_handle,
        addr as u64,
        port as u64,
        addr_len as u32,
        0,
    )?;
    Ok(0)
}

pub fn net_connect(fd: i32, addr_ptr: u64, addr_len: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_connect(fd as u32, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_CONNECT)?;
    let sock = resolve_net_fd(fd, pid)?;
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    let nb_flag = u32::from(sock.nonblock);
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(CONNECT_TIMEOUT_NS);
    loop {
        match dispatch_for(
            sock.owner_pid,
            NET_OP_CONNECT,
            sock.net_handle,
            addr as u64,
            port as u64,
            addr_len as u32,
            nb_flag,
        ) {
            Ok(_) => {
                fs_bridge::notify_io_event(sock.blob_id);
                return Ok(0);
            }
            Err(EINPROGRESS) if sock.nonblock => return Err(EINPROGRESS),
            Err(EAGAIN) if !sock.nonblock => {
                if !wait_net_slice(Some(deadline))? {
                    return Err(ETIMEDOUT);
                }
            }
            Err(err) => {
                fs_bridge::notify_io_event(sock.blob_id);
                return Err(err);
            }
        }
    }
}

pub fn net_listen(fd: i32, backlog: i32) -> Result<i64, i64> {
    if fd < 0 || backlog < 0 {
        return Err(EINVAL);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_listen(fd as u32, backlog, pid).map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_LISTEN)?;
    let sock = resolve_net_fd(fd, pid)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_LISTEN,
        sock.net_handle,
        backlog as u64,
        0,
        0,
        0,
    )?;
    Ok(0)
}

pub fn net_accept(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_accept(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_ACCEPT)?;
    let sock = resolve_net_fd(fd, pid)?;
    let reply = loop {
        match dispatch_for(sock.owner_pid, NET_OP_ACCEPT, sock.net_handle, 0, 0, 0, 0) {
            Ok(r) => break r,
            Err(EAGAIN) if !sock.nonblock => {
                let _ = wait_net_slice(None)?;
            }
            Err(err) => return Err(err),
        }
    };
    let child_handle = reply_u64(&reply, 0) as u32;
    let peer_addr = reply_u32(&reply, 8);
    let peer_port = reply_u16(&reply, 12);
    if let Err(err) = write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port) {
        let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
        return Err(err);
    }
    if !fs_bridge::is_fs_ready() {
        return Ok(child_handle as i64);
    }
    let blob_id = fs_bridge::next_pseudo_blob(fs_bridge::PSEUDO_NET_SOCKET_TAG);
    if fs_bridge::ensure_blob_exists(blob_id).is_err() {
        let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
        return Err(ENOMEM);
    }
    let obj_fd = match OBJECT_TABLE.open(blob_id, open_flags::O_RDWR, 0, 0, pid as u64) {
        Ok(f) => f,
        Err(_) => {
            let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
    };
    {
        let mut map = NET_SOCKET_MAP.lock();
        if map.try_reserve(1).is_err() {
            drop(map);
            let _ = OBJECT_TABLE.close(obj_fd);
            let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
        map.push(NetSocketMeta {
            blob_id,
            net_handle: child_handle,
            owner_pid: sock.owner_pid,
            sock_type: SOCK_STREAM,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    if fs_bridge::process_has_fd_table(pid) {
        let fd_flags = fs_bridge::fd_table_flags(0, open_flags::O_RDWR);
        if let Some(logical_fd) = fs_bridge::install_process_fd(pid, obj_fd as u64, fd_flags) {
            return Ok(logical_fd as i64);
        }
        let _ = fs_bridge::close_object_handle_with_cleanup(obj_fd);
        return Err(ENOMEM);
    }
    Ok(obj_fd as i64)
}

pub fn net_shutdown(fd: i32, how: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_shutdown(fd as u32, how, pid).map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_SHUTDOWN,
        sock.net_handle,
        how as u32 as u64,
        0,
        0,
        0,
    )?;
    fs_bridge::notify_io_event(sock.blob_id);
    Ok(0)
}

pub fn net_getsockname(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getsockname(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    let reply = dispatch_for(
        sock.owner_pid,
        NET_OP_GETSOCKNAME,
        sock.net_handle,
        0,
        0,
        0,
        0,
    )?;
    write_sockaddr_in(
        addr_ptr,
        addr_len_ptr,
        reply_u32(&reply, 8),
        reply_u16(&reply, 12),
    )?;
    Ok(0)
}

pub fn net_getpeername(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getpeername(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    let reply = dispatch_for(
        sock.owner_pid,
        NET_OP_GETPEERNAME,
        sock.net_handle,
        0,
        0,
        0,
        0,
    )?;
    write_sockaddr_in(
        addr_ptr,
        addr_len_ptr,
        reply_u32(&reply, 8),
        reply_u16(&reply, 12),
    )?;
    Ok(0)
}

pub fn net_socketpair(
    domain: i32,
    ty: i32,
    protocol: i32,
    sv_ptr: u64,
    pid: u32,
) -> Result<i64, i64> {
    if domain == AF_UNIX {
        return fs_bridge::fs_socketpair(domain, ty, protocol, sv_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    if sv_ptr == 0 {
        return Err(EFAULT);
    }
    let reply = dispatch_for(
        pid,
        NET_OP_SOCKETPAIR,
        0,
        domain as u32 as u64,
        ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    let a_h = reply_u32(&reply, 0);
    let b_h = reply_u32(&reply, 4);
    let clean_ty = ty & !((fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC) as i32);
    let open_flags_raw = (ty as u32) & (fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC);
    let fd_a = install_net_socket_fd(a_h, pid, clean_ty, open_flags_raw)? as i32;
    let fd_b = match install_net_socket_fd(b_h, pid, clean_ty, open_flags_raw) {
        Ok(f) => f as i32,
        Err(e) => {
            let _ = fs_bridge::fs_close(fd_a as u32, pid);
            return Err(e);
        }
    };
    let fds = [fd_a, fd_b];
    if copy_to_user(
        sv_ptr as *mut u8,
        fds.as_ptr() as *const u8,
        core::mem::size_of_val(&fds),
    )
    .is_err()
    {
        let _ = fs_bridge::fs_close(fd_a as u32, pid);
        let _ = fs_bridge::fs_close(fd_b as u32, pid);
        return Err(EFAULT);
    }
    Ok(0)
}
