use exo_syscall_abi as syscall;

pub const SERVER_ENDPOINT_ID: u64 = 7;
pub const RAW_MSG_SIZE: usize = 240;
pub const IPC_RECV_TIMEOUT_MS: u64 = 5_000;

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

pub const NET_CTRL_DRIVER_INIT: u32 = 0x4F00;
pub const NET_CTRL_RX_RELEASE: u32 = 0x4F01;
pub const NET_CTRL_MAC_QUERY: u32 = 0x4F02;
pub const NET_CTRL_MAC_REPLY: u32 = 0x4F03;

pub const CALL_MAGIC: u32 = 0x4558_4F43;

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

impl NetReply {
    pub const fn ok(status: i64) -> Self {
        Self {
            status,
            payload: [0; 40],
        }
    }

    pub const fn error(status: i64) -> Self {
        Self {
            status,
            payload: [0; 40],
        }
    }

    pub fn with_u64(mut self, offset: usize, value: u64) -> Self {
        self.payload[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        self
    }

    pub fn with_u32(mut self, offset: usize, value: u32) -> Self {
        self.payload[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self
    }

    pub fn with_u16(mut self, offset: usize, value: u16) -> Self {
        self.payload[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        self
    }
}

const _: () = assert!(core::mem::size_of::<NetReply>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DriverInitMsg {
    pub opcode: u32,
    pub pool_count: u32,
    pub rx_base_iova: u64,
    pub tx_base_iova: u64,
    pub hdr_size: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<DriverInitMsg>() == 32);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RxReleaseMsg {
    pub opcode: u32,
    pub count: u32,
    pub pool_idx: [u16; 20],
}

const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawCallHeader {
    pub magic: u32,
    pub payload_len: u32,
    pub cookie: u64,
    pub reply_ep: u64,
}

pub const RAW_CALL_HEADER_SIZE: usize = core::mem::size_of::<RawCallHeader>();

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
    if reply_ep == 0 {
        return syscall::EINVAL;
    }

    let hdr = RawCallHeader {
        magic: CALL_MAGIC,
        payload_len: core::mem::size_of::<NetReply>() as u32,
        cookie,
        reply_ep,
    };
    let mut out = [0u8; RAW_CALL_HEADER_SIZE + core::mem::size_of::<NetReply>()];
    unsafe {
        core::ptr::write_unaligned(out.as_mut_ptr() as *mut RawCallHeader, hdr);
    }
    out[RAW_CALL_HEADER_SIZE..].copy_from_slice(unsafe {
        core::slice::from_raw_parts(
            reply as *const NetReply as *const u8,
            core::mem::size_of::<NetReply>(),
        )
    });

    unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            reply_ep,
            out.as_ptr() as u64,
            out.len() as u64,
            0,
            0,
            0,
        )
    }
}
