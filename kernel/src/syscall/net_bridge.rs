// syscall/net_bridge.rs -- pont BSD sockets -> network_server
//
// Architecture V4: les syscalls reseau Ring 3 ne manipulent pas de memoire
// partagee inter-process. Le noyau copie les petites structures userspace,
// encode un NetMsg fixe (48B), puis effectue un appel IPC raw synchrone vers
// network_server. Les buffers applicatifs longs ne sont pas transmis au serveur
// dans cette phase; le serveur gere l'etat socket et retourne des tailles.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::errno::EMSGSIZE;
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};

pub const NET_OP_OPEN: u32 = 0x4E00;
pub const NET_OP_CONNECT: u32 = 0x4E01;
pub const NET_OP_BIND: u32 = 0x4E02;
pub const NET_OP_LISTEN: u32 = 0x4E03;
pub const NET_OP_ACCEPT: u32 = 0x4E04;
pub const NET_OP_SENDTO: u32 = 0x4E05;
pub const NET_OP_RECVFROM: u32 = 0x4E06;
pub const NET_OP_SENDMSG: u32 = 0x4E07;
pub const NET_OP_RECVMSG: u32 = 0x4E08;
pub const NET_OP_SHUTDOWN: u32 = 0x4E09;
pub const NET_OP_GETSOCKNAME: u32 = 0x4E0A;
pub const NET_OP_SOCKETPAIR: u32 = 0x4E0B;
pub const NET_OP_SETSOCKOPT: u32 = 0x4E0C;
pub const NET_OP_GETSOCKOPT: u32 = 0x4E0D;
pub const NET_OP_CLOSE: u32 = 0x4E0E;
pub const NET_OP_GETPEERNAME: u32 = 0x4E0F;

const AF_UNIX: i32 = 1;
const AF_INET: u16 = 2;
const SOCKADDR_IN_LEN: usize = 16;
const IOV_MAX: u64 = 1024;
const NET_HANDLE_TAG: u32 = 0x4000_0000;
const NET_HANDLE_TAG_MASK: u32 = 0xf000_0000;
const NET_INLINE_DATA_MAX: usize = 128;
const NET_REPLY_DATA_OFFSET: usize = core::mem::size_of::<NetReply>();

const EAGAIN: i64 = -11;
const EBADF: i64 = -9;
const EFAULT: i64 = -14;
const EINVAL: i64 = -22;
const ENOSYS: i64 = -38;
const ENOTSUP: i64 = -95;
const ENETDOWN: i64 = -100;
const ETIMEDOUT: i64 = -110;
const CONNECT_TIMEOUT_NS: u64 = 10_000_000_000;
const CONNECT_RETRY_NAP_NS: u64 = 10_000_000;

static NET_READY: AtomicBool = AtomicBool::new(false);
static NETWORK_ENDPOINT: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetMsg {
    pub opcode: u32,
    pub sender_pid: u32,
    pub fd: u32,
    pub _pad0: u32,
    pub arg1: u64,
    pub arg2: u64,
    pub arg3: u32,
    pub arg4: u32,
    pub _reserved: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<NetMsg>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetReply {
    pub status: i64,
    pub payload: [u8; 40],
}

const _: () = assert!(core::mem::size_of::<NetReply>() == 48);

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

/// Active le pont reseau apres l'initialisation de base du noyau.
///
/// # Safety
/// Appeler une seule fois depuis le chemin boot BSP, apres l'initialisation IPC.
pub unsafe fn net_bridge_preinit() {
    NET_READY.store(true, Ordering::Release);
}

#[inline]
pub fn bridge_result(result: Result<i64, i64>) -> i64 {
    match result {
        Ok(value) => value,
        Err(errno) => errno,
    }
}

fn current_pid() -> u32 {
    crate::syscall::fast_path::syscall_current_pid()
}

fn network_endpoint() -> Result<EndpointId, i64> {
    if !NET_READY.load(Ordering::Acquire) {
        return Err(ENOSYS);
    }

    let cached = NETWORK_ENDPOINT.load(Ordering::Acquire);
    if cached != 0 {
        return EndpointId::new(cached).ok_or(ENETDOWN);
    }

    let endpoint = crate::ipc::endpoint::lookup_endpoint(b"network_server").ok_or(ENETDOWN)?;
    NETWORK_ENDPOINT.store(endpoint.get(), Ordering::Release);
    Ok(endpoint)
}

fn ipc_errno(err: IpcError) -> i64 {
    match err {
        IpcError::MessageTooLarge => EMSGSIZE,
        IpcError::WouldBlock | IpcError::QueueEmpty | IpcError::QueueFull | IpcError::Full => -11,
        IpcError::OutOfResources | IpcError::ResourceExhausted | IpcError::ShmPoolFull => -12,
        IpcError::NotFound
        | IpcError::EndpointNotFound
        | IpcError::ChannelClosed
        | IpcError::Closed
        | IpcError::ConnRefused => ENETDOWN,
        IpcError::Timeout => -110,
        IpcError::NullEndpoint
        | IpcError::InvalidEndpoint
        | IpcError::InvalidParam
        | IpcError::Invalid
        | IpcError::InvalidArgument
        | IpcError::InvalidHandle
        | IpcError::ProtocolError
        | IpcError::HandshakeFailed
        | IpcError::OutOfOrder => EINVAL,
        IpcError::PermissionDenied => -13,
        _ => ENETDOWN,
    }
}

fn call_network(msg: &NetMsg) -> Result<NetReply, i64> {
    let mut reply_buf = [0u8; core::mem::size_of::<NetReply>()];
    let n = call_network_raw(msg, &[], &mut reply_buf)?;
    if n < core::mem::size_of::<NetReply>() {
        return Err(EINVAL);
    }
    Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const NetReply) })
}

fn call_network_raw(msg: &NetMsg, data: &[u8], reply_buf: &mut [u8]) -> Result<usize, i64> {
    if data.len() > NET_INLINE_DATA_MAX {
        return Err(EMSGSIZE);
    }
    let endpoint = network_endpoint()?;
    let mut request = [0u8; core::mem::size_of::<NetMsg>() + NET_INLINE_DATA_MAX];
    request[..core::mem::size_of::<NetMsg>()].copy_from_slice(unsafe {
        core::slice::from_raw_parts(
            msg as *const NetMsg as *const u8,
            core::mem::size_of::<NetMsg>(),
        )
    });
    request[core::mem::size_of::<NetMsg>()..core::mem::size_of::<NetMsg>() + data.len()]
        .copy_from_slice(data);
    match crate::ipc::rpc::call_raw(
        endpoint,
        &request[..core::mem::size_of::<NetMsg>() + data.len()],
        reply_buf,
    ) {
        Ok(n) if n >= core::mem::size_of::<NetReply>() => Ok(n),
        Ok(_) => Err(EINVAL),
        Err(err) => Err(ipc_errno(err)),
    }
}

fn dispatch(
    opcode: u32,
    fd: u32,
    arg1: u64,
    arg2: u64,
    arg3: u32,
    arg4: u32,
) -> Result<NetReply, i64> {
    let msg = make_msg(opcode, fd, arg1, arg2, arg3, arg4);
    let reply = call_network(&msg)?;
    if reply.status < 0 {
        Err(reply.status)
    } else {
        Ok(reply)
    }
}

fn make_msg(opcode: u32, fd: u32, arg1: u64, arg2: u64, arg3: u32, arg4: u32) -> NetMsg {
    NetMsg {
        opcode,
        sender_pid: current_pid(),
        fd,
        _pad0: 0,
        arg1,
        arg2,
        arg3,
        arg4,
        _reserved: [0; 8],
    }
}

fn read_sockaddr_in(addr_ptr: u64, addr_len: u64) -> Result<(u32, u16), i64> {
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

fn write_sockaddr_in(addr_ptr: u64, addr_len_ptr: u64, addr: u32, port: u16) -> Result<(), i64> {
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

fn reply_u64(reply: &NetReply, offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&reply.payload[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

fn reply_u32(reply: &NetReply, offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&reply.payload[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}

fn reply_u16(reply: &NetReply, offset: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&reply.payload[offset..offset + 2]);
    u16::from_le_bytes(bytes)
}

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
        if iov.iov_len > usize::MAX as u64 {
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

fn copy_msghdr_iov_to_inline(msg: &LinuxMsghdr, out: &mut [u8]) -> Result<usize, i64> {
    let mut written = 0usize;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(EFAULT)?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| EFAULT)?;
        if iov.iov_len > out.len().saturating_sub(written) as u64 {
            return Err(EMSGSIZE);
        }
        let chunk = iov.iov_len as usize;
        if chunk != 0 {
            if iov.iov_base == 0 {
                return Err(EFAULT);
            }
            if copy_from_user(
                out[written..].as_mut_ptr(),
                iov.iov_base as *const u8,
                chunk,
            )
            .is_err()
            {
                return Err(EFAULT);
            }
            written += chunk;
        }
        idx += 1;
    }
    Ok(written)
}

fn copy_inline_to_msghdr_iov(msg: &LinuxMsghdr, data: &[u8]) -> Result<(), i64> {
    let mut copied = 0usize;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen && copied < data.len() {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(EFAULT)?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| EFAULT)?;
        let chunk = (data.len() - copied).min(iov.iov_len as usize);
        if chunk != 0 {
            if iov.iov_base == 0 {
                return Err(EFAULT);
            }
            if copy_to_user(
                iov.iov_base as *mut u8,
                data[copied..copied + chunk].as_ptr(),
                chunk,
            )
            .is_err()
            {
                return Err(EFAULT);
            }
            copied += chunk;
        }
        idx += 1;
    }
    if copied != data.len() {
        return Err(EFAULT);
    }
    Ok(())
}

fn msghdr_peer(msg: &LinuxMsghdr) -> Result<(u32, u16), i64> {
    if msg.msg_name == 0 {
        return Ok((0, 0));
    }
    read_sockaddr_in(msg.msg_name, msg.msg_namelen as u64)
}

pub fn net_socket(domain: i32, ty: i32, protocol: i32) -> Result<i64, i64> {
    let reply = dispatch(
        NET_OP_OPEN,
        0,
        domain as u32 as u64,
        ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    Ok(reply_u64(&reply, 0) as i64)
}

pub fn socket_handle_from_raw(raw: u64) -> Option<i32> {
    if raw > i32::MAX as u64 {
        return None;
    }
    let fd = raw as u32;
    (fd & NET_HANDLE_TAG_MASK == NET_HANDLE_TAG).then_some(fd as i32)
}

pub fn net_close(fd: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    dispatch(NET_OP_CLOSE, fd as u32, 0, 0, 0, 0)?;
    Ok(0)
}

pub fn net_bind(fd: i32, addr_ptr: u64, addr_len: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    dispatch(
        NET_OP_BIND,
        fd as u32,
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
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(CONNECT_TIMEOUT_NS);
    loop {
        match dispatch(
            NET_OP_CONNECT,
            fd as u32,
            addr as u64,
            port as u64,
            addr_len as u32,
            0,
        ) {
            Ok(_) => return Ok(0),
            Err(EAGAIN) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    return Err(ETIMEDOUT);
                }
                if !crate::scheduler::timer::sleep_ns(CONNECT_RETRY_NAP_NS) {
                    crate::syscall::fast_path::sys_sched_yield();
                }
            }
            Err(err) => return Err(err),
        }
    }
}

pub fn net_listen(fd: i32, backlog: i32) -> Result<i64, i64> {
    if fd < 0 || backlog < 0 {
        return Err(EINVAL);
    }
    dispatch(NET_OP_LISTEN, fd as u32, backlog as u64, 0, 0, 0)?;
    Ok(0)
}

pub fn net_accept(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let reply = dispatch(NET_OP_ACCEPT, fd as u32, 0, 0, 0, 0)?;
    let peer_addr = reply_u32(&reply, 8);
    let peer_port = reply_u16(&reply, 12);
    write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port)?;
    Ok(reply_u64(&reply, 0) as i64)
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
    if len != 0 && buf_ptr == 0 {
        return Err(EFAULT);
    }
    let (addr, port) = if addr_ptr != 0 {
        read_sockaddr_in(addr_ptr, addr_len)?
    } else {
        (0, 0)
    };
    if len > NET_INLINE_DATA_MAX {
        return Err(EMSGSIZE);
    }
    let mut data = [0u8; NET_INLINE_DATA_MAX];
    if len != 0 && copy_from_user(data.as_mut_ptr(), buf_ptr as *const u8, len).is_err() {
        return Err(EFAULT);
    }
    let msg = make_msg(
        NET_OP_SENDTO,
        fd as u32,
        len as u64,
        addr as u64,
        port as u32,
        flags,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let _ = call_network_raw(&msg, &data[..len], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
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
    let request_len = len.min(NET_INLINE_DATA_MAX);
    let msg = make_msg(NET_OP_RECVFROM, fd as u32, request_len as u64, 0, flags, 0);
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let n_raw = call_network_raw(&msg, &[], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
    let n = reply.status.max(0) as usize;
    let data_len = n.min(len).min(n_raw.saturating_sub(NET_REPLY_DATA_OFFSET));
    if data_len != 0 {
        if buf_ptr == 0 {
            return Err(EFAULT);
        }
        if copy_to_user(
            buf_ptr as *mut u8,
            reply_raw[NET_REPLY_DATA_OFFSET..].as_ptr(),
            data_len,
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    let peer_addr = reply_u32(&reply, 8);
    let peer_port = reply_u16(&reply, 12);
    write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port)?;
    Ok(n as i64)
}

pub fn net_sendmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let len = msghdr_total_iov_len(&msg)?;
    let (addr, port) = msghdr_peer(&msg)?;
    if len > NET_INLINE_DATA_MAX {
        return Err(EMSGSIZE);
    }
    let mut data = [0u8; NET_INLINE_DATA_MAX];
    let copied = copy_msghdr_iov_to_inline(&msg, &mut data)?;
    let net_msg = make_msg(
        NET_OP_SENDTO,
        fd as u32,
        copied as u64,
        addr as u64,
        port as u32,
        flags,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let _ = call_network_raw(&net_msg, &data[..copied], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
    Ok(reply.status)
}

pub fn net_recvmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let mut msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let len = msghdr_total_iov_len(&msg)?;
    let request_len = len.min(NET_INLINE_DATA_MAX);
    let net_msg = make_msg(NET_OP_RECVFROM, fd as u32, request_len as u64, 0, flags, 0);
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let n_raw = call_network_raw(&net_msg, &[], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
    let n = reply.status.max(0) as usize;
    let data_len = n.min(len).min(n_raw.saturating_sub(NET_REPLY_DATA_OFFSET));
    copy_inline_to_msghdr_iov(
        &msg,
        &reply_raw[NET_REPLY_DATA_OFFSET..NET_REPLY_DATA_OFFSET + data_len],
    )?;
    if msg.msg_name != 0 {
        let peer_addr = reply_u32(&reply, 8);
        let peer_port = reply_u16(&reply, 12);
        write_sockaddr_in(msg.msg_name, 0, peer_addr, peer_port)?;
        msg.msg_namelen = SOCKADDR_IN_LEN as u32;
    }
    msg.msg_flags = 0;
    write_user_typed::<LinuxMsghdr>(msg_ptr, msg).map_err(|_| EFAULT)?;
    Ok(n as i64)
}

pub fn net_shutdown(fd: i32, how: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    dispatch(NET_OP_SHUTDOWN, fd as u32, how as u32 as u64, 0, 0, 0)?;
    Ok(0)
}

pub fn net_getsockname(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let reply = dispatch(NET_OP_GETSOCKNAME, fd as u32, 0, 0, 0, 0)?;
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
    let reply = dispatch(NET_OP_GETPEERNAME, fd as u32, 0, 0, 0, 0)?;
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
        return crate::syscall::fs_bridge::fs_socketpair(domain, ty, protocol, sv_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    if sv_ptr == 0 {
        return Err(EFAULT);
    }
    let reply = dispatch(
        NET_OP_SOCKETPAIR,
        0,
        domain as u32 as u64,
        ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    let a = reply_u64(&reply, 0);
    let b = reply_u64(&reply, 8);
    let fds = [a as i32, b as i32];
    if copy_to_user(
        sv_ptr as *mut u8,
        fds.as_ptr() as *const u8,
        core::mem::size_of_val(&fds),
    )
    .is_err()
    {
        return Err(EFAULT);
    }
    Ok(0)
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
    if optlen != 0 && optval == 0 {
        return Err(EFAULT);
    }
    dispatch(
        NET_OP_SETSOCKOPT,
        fd as u32,
        level as u32 as u64,
        optname as u32 as u64,
        optlen,
        0,
    )?;
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
    let reply = dispatch(
        NET_OP_GETSOCKOPT,
        fd as u32,
        level as u32 as u64,
        optname as u32 as u64,
        0,
        0,
    )?;
    if optval != 0 {
        let value = reply_u32(&reply, 16);
        if copy_to_user(
            optval as *mut u8,
            &value as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    if optlen_ptr != 0 {
        let len = core::mem::size_of::<u32>() as u32;
        if copy_to_user(
            optlen_ptr as *mut u8,
            &len as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    Ok(0)
}
