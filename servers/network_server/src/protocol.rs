use exo_syscall_abi as syscall;

pub const SERVER_ENDPOINT_ID: u64 = 7;
pub const RAW_MSG_SIZE: usize = syscall::IPC_ENVELOPE_SIZE;
// The timeout value is encoded in the same ABI word as IPC_FLAG_TIMEOUT, so it
// must stay even: `IPC_FLAG_TIMEOUT | 20` decodes to a real 20 ms timeout.
pub const IPC_RECV_TIMEOUT_MS: u64 = 20;

const _: () = assert!(RAW_MSG_SIZE == syscall::IPC_ENVELOPE_SIZE);

pub use syscall::{
    DriverCtrlMsg, DriverInitMsg, MacReplyMsg, NetMsg, NetReply, RxPacketRef, RxReadyMsg,
    RxReleaseMsg, TxCompleteMsg, TxSubmitMsg, NET_CALL_MAGIC as CALL_MAGIC, NET_CTRL_DRIVER_INIT,
    NET_CTRL_MAC_QUERY, NET_CTRL_MAC_REPLY, NET_CTRL_RX_READY, NET_CTRL_RX_RELEASE,
    NET_CTRL_TX_COMPLETE, NET_CTRL_TX_SUBMIT, NET_DATA_WINDOW_SIZE, NET_FLAG_USE_DATA_WINDOW,
    NET_OP_ACCEPT, NET_OP_BIND, NET_OP_CLOSE, NET_OP_CONNECT, NET_OP_GETPEERNAME,
    NET_OP_GETSOCKNAME, NET_OP_GETSOCKOPT, NET_OP_HANDSHAKE, NET_OP_LISTEN, NET_OP_OPEN,
    NET_OP_POLL, NET_OP_RECVFROM, NET_OP_RECVMSG, NET_OP_SENDMSG, NET_OP_SENDTO,
    NET_OP_SETSOCKOPT, NET_OP_SHUTDOWN, NET_OP_SOCKETPAIR,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawCallHeader {
    pub magic: u32,
    pub payload_len: u32,
    pub cookie: u64,
    pub reply_ep: u64,
}

pub const RAW_CALL_HEADER_SIZE: usize = core::mem::size_of::<RawCallHeader>();
pub const NET_INLINE_DATA_MAX: usize =
    RAW_MSG_SIZE - RAW_CALL_HEADER_SIZE - core::mem::size_of::<NetMsg>();

pub struct RawCall<'a> {
    pub payload: &'a [u8],
    pub cookie: u64,
    pub reply_ep: u64,
}

pub fn parse_raw_call(buf: &[u8]) -> Option<RawCall<'_>> {
    if buf.len() < RAW_CALL_HEADER_SIZE {
        return None;
    }
    let hdr: RawCallHeader =
        unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const RawCallHeader) };
    if hdr.magic != CALL_MAGIC {
        return None;
    }
    let payload_len = hdr.payload_len as usize;
    if buf.len() < RAW_CALL_HEADER_SIZE + payload_len {
        return None;
    }
    Some(RawCall {
        payload: &buf[RAW_CALL_HEADER_SIZE..RAW_CALL_HEADER_SIZE + payload_len],
        cookie: hdr.cookie,
        reply_ep: hdr.reply_ep,
    })
}

pub fn parse_net_msg(payload: &[u8]) -> Option<NetMsg> {
    if payload.len() < core::mem::size_of::<NetMsg>() {
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(payload.as_ptr() as *const NetMsg) })
}

pub fn parse_driver_ctrl(buf: &[u8]) -> Option<DriverCtrlMsg> {
    if buf.len() < core::mem::size_of::<DriverCtrlMsg>() {
        return None;
    }
    let ctrl = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const DriverCtrlMsg) };
    match ctrl.msg_type {
        NET_CTRL_MAC_REPLY | NET_CTRL_RX_READY | NET_CTRL_TX_COMPLETE => Some(ctrl),
        _ => None,
    }
}

pub fn register_endpoint() {
    let name = b"network_server";
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

pub fn recv_raw(buf: &mut [u8; RAW_MSG_SIZE]) -> Result<usize, i64> {
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            SERVER_ENDPOINT_ID,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    };
    if rc == syscall::ETIMEDOUT {
        return Ok(0);
    }
    if rc < 0 {
        return Err(rc);
    }
    Ok(rc as usize)
}

pub fn send_rpc_reply(reply_ep: u64, cookie: u64, reply: &NetReply) -> i64 {
    send_rpc_reply_with_data(reply_ep, cookie, reply, &[])
}

pub fn send_rpc_reply_with_data(reply_ep: u64, cookie: u64, reply: &NetReply, data: &[u8]) -> i64 {
    if reply_ep == 0 {
        return syscall::EINVAL;
    }
    if data.len() > NET_INLINE_DATA_MAX {
        return syscall::EMSGSIZE;
    }

    let hdr = RawCallHeader {
        magic: CALL_MAGIC,
        payload_len: (core::mem::size_of::<NetReply>() + data.len()) as u32,
        cookie,
        reply_ep,
    };
    let mut out = [0u8; RAW_MSG_SIZE];
    unsafe {
        core::ptr::write_unaligned(out.as_mut_ptr() as *mut RawCallHeader, hdr);
    }
    let reply_start = RAW_CALL_HEADER_SIZE;
    let reply_end = reply_start + core::mem::size_of::<NetReply>();
    out[reply_start..reply_end].copy_from_slice(unsafe {
        core::slice::from_raw_parts(
            reply as *const NetReply as *const u8,
            core::mem::size_of::<NetReply>(),
        )
    });
    out[reply_end..reply_end + data.len()].copy_from_slice(data);

    unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            reply_ep,
            out.as_ptr() as u64,
            (RAW_CALL_HEADER_SIZE + core::mem::size_of::<NetReply>() + data.len()) as u64,
            0,
            0,
            0,
        )
    }
}
