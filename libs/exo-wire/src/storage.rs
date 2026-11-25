//! Canonical Block Storage IPC wire structures (`S31`, `S33`).

use crate::ipc::{IPC_ENVELOPE_SIZE, IPC_INLINE_PAYLOAD_SIZE};

pub const BLOCK_OP_READ: u32 = 0x3001;
pub const BLOCK_OP_WRITE: u32 = 0x3002;
pub const BLOCK_OP_FLUSH: u32 = 0x3003;
pub const BLOCK_OP_GET_INFO: u32 = 0x3004;
pub const BLOCK_DEFAULT_SIZE: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockCmdWire {
    pub opcode: u32,
    pub flags: u32,
    pub lba: u64,
    pub count: u32,
    pub _pad: u32,
    pub dma_phys: u64,
    pub cap_token: [u8; 20],
    pub _reserved: [u8; 8],
}

impl BlockCmdWire {
    #[inline]
    pub const fn zeroed() -> Self {
        Self {
            opcode: 0,
            flags: 0,
            lba: 0,
            count: 0,
            _pad: 0,
            dma_phys: 0,
            cap_token: [0u8; 20],
            _reserved: [0u8; 8],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BlockReplyWire {
    pub status: i64,
    pub lba: u64,
    pub count: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub _reserved: [u8; 32],
}

impl BlockReplyWire {
    #[inline]
    pub const fn ok(lba: u64, count: u32, block_size: u32, total_blocks: u64) -> Self {
        Self {
            status: 0,
            lba,
            count,
            block_size,
            total_blocks,
            _reserved: [0u8; 32],
        }
    }

    #[inline]
    pub const fn error(status: i64) -> Self {
        Self {
            status,
            lba: 0,
            count: 0,
            block_size: 0,
            total_blocks: 0,
            _reserved: [0u8; 32],
        }
    }
}

#[repr(C)]
pub struct BlockIpcRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub cmd: BlockCmdWire,
    pub _pad: [u8; IPC_INLINE_PAYLOAD_SIZE - core::mem::size_of::<BlockCmdWire>()],
}

const _: () = assert!(core::mem::size_of::<BlockIpcRequest>() == IPC_ENVELOPE_SIZE);

#[repr(C)]
pub struct BlockIpcReply {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply: BlockReplyWire,
    pub _pad: [u8; IPC_INLINE_PAYLOAD_SIZE - core::mem::size_of::<BlockReplyWire>()],
}

const _: () = assert!(core::mem::size_of::<BlockIpcReply>() == IPC_ENVELOPE_SIZE);
