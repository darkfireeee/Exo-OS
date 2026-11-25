//! # syscall/net_bridge/rpc.rs — Ring 0 <-> `network_server` IPC transport helpers

use core::sync::atomic::Ordering;

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::errno::EMSGSIZE;
use crate::syscall::fs_bridge;
use exo_abi::errno::{EINVAL, ENETDOWN, ENOSYS, ETIMEDOUT};

use super::{
    data_window, NetMsg, NetReply, NETWORK_ENDPOINT, NET_INLINE_DATA_MAX, NET_READY,
    NET_WAIT_SLICE_NS,
};

pub(in crate::syscall::net_bridge) fn current_pid() -> u32 {
    crate::syscall::fast_path::syscall_current_pid()
}

pub(in crate::syscall::net_bridge) fn network_endpoint() -> Result<EndpointId, i64> {
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

pub(in crate::syscall::net_bridge) fn ipc_errno(err: IpcError) -> i64 {
    match err {
        IpcError::MessageTooLarge => EMSGSIZE,
        IpcError::WouldBlock | IpcError::QueueEmpty | IpcError::QueueFull | IpcError::Full => -11,
        IpcError::OutOfResources | IpcError::ResourceExhausted | IpcError::ShmPoolFull => -12,
        IpcError::NotFound
        | IpcError::EndpointNotFound
        | IpcError::ChannelClosed
        | IpcError::Closed
        | IpcError::ConnRefused => {
            NETWORK_ENDPOINT.store(0, Ordering::Release);
            *data_window::NET_DATA_WINDOW_CACHE.lock() = None;
            ENETDOWN
        }
        IpcError::Timeout => ETIMEDOUT,
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

pub(in crate::syscall::net_bridge) fn make_msg_for(
    sender_pid: u32,
    opcode: u32,
    fd: u32,
    arg1: u64,
    arg2: u64,
    arg3: u32,
    arg4: u32,
) -> NetMsg {
    NetMsg {
        opcode,
        sender_pid,
        fd,
        _pad0: 0,
        arg1,
        arg2,
        arg3,
        arg4,
        _reserved: [0; 8],
    }
}

pub(in crate::syscall::net_bridge) fn call_network(msg: &NetMsg) -> Result<NetReply, i64> {
    let mut reply_buf = [0u8; core::mem::size_of::<NetReply>()];
    let n = call_network_raw(msg, &[], &mut reply_buf)?;
    if n < core::mem::size_of::<NetReply>() {
        return Err(EINVAL);
    }
    Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const NetReply) })
}

pub(in crate::syscall::net_bridge) fn call_network_raw(
    msg: &NetMsg,
    data: &[u8],
    reply_buf: &mut [u8],
) -> Result<usize, i64> {
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

pub(in crate::syscall::net_bridge) fn dispatch_for(
    sender_pid: u32,
    opcode: u32,
    fd: u32,
    arg1: u64,
    arg2: u64,
    arg3: u32,
    arg4: u32,
) -> Result<NetReply, i64> {
    let msg = make_msg_for(sender_pid, opcode, fd, arg1, arg2, arg3, arg4);
    let reply = call_network(&msg)?;
    if reply.status < 0 {
        Err(reply.status)
    } else {
        Ok(reply)
    }
}

pub(in crate::syscall::net_bridge) fn reply_u64(reply: &NetReply, offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&reply.payload[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

pub(in crate::syscall::net_bridge) fn reply_u32(reply: &NetReply, offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&reply.payload[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}

pub(in crate::syscall::net_bridge) fn reply_u16(reply: &NetReply, offset: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&reply.payload[offset..offset + 2]);
    u16::from_le_bytes(bytes)
}

#[inline]
pub(in crate::syscall::net_bridge) fn check_net_permission(syscall_no: u64) -> Result<(), i64> {
    let caller_pid = current_pid();
    if caller_pid != 0 {
        let ctx = crate::security::zero_trust::context_for_caller(caller_pid, 0);
        if crate::security::zero_trust::verify_syscall(&ctx, syscall_no).is_err() {
            return Err(-1);
        }
    }
    Ok(())
}

pub(in crate::syscall::net_bridge) fn wait_net_slice(
    deadline_ns: Option<u64>,
) -> Result<bool, i64> {
    let gen = fs_bridge::io_wait_gen();
    fs_bridge::wait_for_io_event(gen, deadline_ns, NET_WAIT_SLICE_NS).map_err(|e| e.to_errno())
}
