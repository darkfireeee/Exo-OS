//! # syscall/table/fs/sockets.rs — Handlers de l'interface réseau BSD Sockets

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::IO_BUF_MAX;

/// `socket(domain, type, protocol)`.
pub fn sys_socket(domain: u64, ty: u64, protocol: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SOCKET);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_socket(
        domain as i32,
        ty as i32,
        protocol as i32,
    ))
}

/// `connect(fd, sockaddr*, addrlen)`.
pub fn sys_connect(fd: u64, addr_ptr: u64, addr_len: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CONNECT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_connect(fd as i32, addr_ptr, addr_len))
}

/// `bind(fd, sockaddr*, addrlen)`.
pub fn sys_bind(fd: u64, addr_ptr: u64, addr_len: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_BIND);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_bind(fd as i32, addr_ptr, addr_len))
}

/// `listen(fd, backlog)`.
pub fn sys_listen(fd: u64, backlog: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LISTEN);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_listen(fd as i32, backlog as i32))
}

/// `accept(fd, sockaddr*, socklen_t*)`.
pub fn sys_accept(fd: u64, addr_ptr: u64, addr_len_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_ACCEPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_accept(fd as i32, addr_ptr, addr_len_ptr))
}

/// `sendto(fd, buf, len, flags, sockaddr*, addrlen)`.
pub fn sys_sendto(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    flags: u64,
    addr_ptr: u64,
    addr_len: u64,
) -> i64 {
    stat_inc(SYS_SENDTO);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_sendto(
        fd as i32,
        buf_ptr,
        len as usize,
        flags as u32,
        addr_ptr,
        addr_len,
    ))
}

/// `recvfrom(fd, buf, len, flags, sockaddr*, socklen_t*)`.
pub fn sys_recvfrom(
    fd: u64,
    buf_ptr: u64,
    len: u64,
    flags: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> i64 {
    stat_inc(SYS_RECVFROM);
    if len as usize as u64 != len || len as usize > IO_BUF_MAX {
        return E2BIG;
    }
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_recvfrom(
        fd as i32,
        buf_ptr,
        len as usize,
        flags as u32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `sendmsg(fd, msghdr*, flags)`.
pub fn sys_sendmsg(fd: u64, msg_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SENDMSG);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_sendmsg(fd as i32, msg_ptr, flags as u32))
}

/// `recvmsg(fd, msghdr*, flags)`.
pub fn sys_recvmsg(fd: u64, msg_ptr: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RECVMSG);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_recvmsg(fd as i32, msg_ptr, flags as u32))
}

/// `shutdown(fd, how)`.
pub fn sys_shutdown(fd: u64, how: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SHUTDOWN);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_shutdown(fd as i32, how as i32))
}

/// `getsockname(fd, sockaddr*, socklen_t*)`.
pub fn sys_getsockname(
    fd: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETSOCKNAME);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getsockname(
        fd as i32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `getpeername(fd, sockaddr*, socklen_t*)`.
pub fn sys_getpeername(
    fd: u64,
    addr_ptr: u64,
    addr_len_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETPEERNAME);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getpeername(
        fd as i32,
        addr_ptr,
        addr_len_ptr,
    ))
}

/// `socketpair(domain, type, protocol, sv)`.
pub fn sys_socketpair(domain: u64, ty: u64, protocol: u64, sv_ptr: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SOCKETPAIR);
    use crate::syscall::net_bridge;
    let pid = current_pid_u32();
    net_bridge::bridge_result(net_bridge::net_socketpair(
        domain as i32,
        ty as i32,
        protocol as i32,
        sv_ptr,
        pid,
    ))
}

/// `setsockopt(fd, level, optname, optval, optlen)`.
pub fn sys_setsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval: u64,
    optlen: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SETSOCKOPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_setsockopt(
        fd as i32,
        level as i32,
        optname as i32,
        optval,
        optlen as u32,
    ))
}

/// `getsockopt(fd, level, optname, optval, optlen*)`.
pub fn sys_getsockopt(
    fd: u64,
    level: u64,
    optname: u64,
    optval: u64,
    optlen_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_GETSOCKOPT);
    use crate::syscall::net_bridge;
    net_bridge::bridge_result(net_bridge::net_getsockopt(
        fd as i32,
        level as i32,
        optname as i32,
        optval,
        optlen_ptr,
    ))
}
