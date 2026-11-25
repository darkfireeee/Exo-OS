//! Canonical ExoNet V5 BSD-socket RPC (`net_bridge` ↔ `network_server`) and
//! Ring 1 NIC driver control-plane (`e1000`, `virtio_net`, `loopback` ↔ `network_server`) wire types.

use crate::ipc::{IPC_ENVELOPE_SIZE, IPC_HEADER_SIZE, IPC_INLINE_PAYLOAD_SIZE};

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
pub const NET_OP_HANDSHAKE: u32 = 0x4E10;
pub const NET_OP_POLL: u32 = 0x4E11;

pub const NET_FLAG_USE_DATA_WINDOW: u32 = 0x8000_0000;
pub const NET_DATA_WINDOW_SIZE: usize = 16384;

pub const NET_CTRL_DRIVER_INIT: u32 = 0x4F00;
pub const NET_CTRL_RX_RELEASE: u32 = 0x4F01;
pub const NET_CTRL_MAC_QUERY: u32 = 0x4F02;
pub const NET_CTRL_MAC_REPLY: u32 = 0x4F03;
pub const NET_CTRL_RX_READY: u32 = 0x4F04;
pub const NET_CTRL_TX_SUBMIT: u32 = 0x4F05;
pub const NET_CTRL_TX_COMPLETE: u32 = 0x4F06;

pub const NET_CALL_MAGIC: u32 = 0x4558_4F43;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug)]
pub struct NetReply {
    pub status: i64,
    pub payload: [u8; 40],
}

impl NetReply {
    #[inline]
    pub const fn ok(status: i64) -> Self {
        Self {
            status,
            payload: [0; 40],
        }
    }

    #[inline]
    pub const fn error(status: i64) -> Self {
        Self {
            status,
            payload: [0; 40],
        }
    }

    #[inline]
    pub fn with_u64(mut self, offset: usize, value: u64) -> Self {
        self.payload[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        self
    }

    #[inline]
    pub fn with_u32(mut self, offset: usize, value: u32) -> Self {
        self.payload[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self
    }

    #[inline]
    pub fn with_u16(mut self, offset: usize, value: u16) -> Self {
        self.payload[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        self
    }
}

const _: () = assert!(core::mem::size_of::<NetReply>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriverInitMsg {
    pub opcode: u32,
    pub pool_count: u32,
    pub rx_base_iova: u64,
    pub tx_base_iova: u64,
    pub hdr_size: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<DriverInitMsg>() == 32);

/// Preserves the audited 48-byte `EXONET_V4_AUDIT.md` DMA release layout.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RxReleaseMsg {
    pub opcode: u32,
    pub count: u32,
    pub pool_idx: [u16; 20],
}

const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxPacketRef {
    pub pool_idx: u16,
    pub len: u16,
}

const _: () = assert!(core::mem::size_of::<RxPacketRef>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RxReadyMsg {
    pub opcode: u32,
    pub count: u32,
    pub entries: [RxPacketRef; 20],
}

const _: () = assert!(core::mem::size_of::<RxReadyMsg>() == 88);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxSubmitMsg {
    pub opcode: u32,
    pub pool_idx: u16,
    pub len: u16,
}

const _: () = assert!(core::mem::size_of::<TxSubmitMsg>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TxCompleteMsg {
    pub opcode: u32,
    pub count: u32,
    pub pool_idx: [u16; 20],
}

const _: () = assert!(core::mem::size_of::<TxCompleteMsg>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacReplyMsg {
    pub opcode: u32,
    pub mac: [u8; 6],
    pub _pad: [u8; 2],
}

const _: () = assert!(core::mem::size_of::<MacReplyMsg>() == 12);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DriverCtrlMsg {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; IPC_INLINE_PAYLOAD_SIZE],
}

impl DriverCtrlMsg {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

const _: () = assert!(core::mem::size_of::<DriverCtrlMsg>() == IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(DriverCtrlMsg, payload) == IPC_HEADER_SIZE);
