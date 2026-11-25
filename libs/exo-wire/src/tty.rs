//! Canonical TTY wire protocol (`kernel/fs_bridge` ↔ `tty_server`).

use crate::ipc::IPC_KERNEL_MAX_MSG_SIZE;

pub const TTY_MSG_INPUT_BYTE: u32 = 0x130;
pub const TTY_MSG_READ_LINE: u32 = 0x131;
pub const TTY_MSG_WRITE: u32 = 0x132;
pub const TTY_MSG_IOCTL: u32 = 0x133;

pub const TTY_LINE_MAX: usize = 184;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TtyRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub a: u64,
    pub b: u64,
    pub data: [u8; TTY_LINE_MAX],
}

impl TtyRequest {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            reply_endpoint: 0,
            a: 0,
            b: 0,
            data: [0; TTY_LINE_MAX],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TtyReply {
    pub status: i64,
    pub signal: u32,
    pub len: u32,
    pub data: [u8; TTY_LINE_MAX],
}

impl TtyReply {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            status: 0,
            signal: 0,
            len: 0,
            data: [0; TTY_LINE_MAX],
        }
    }
}

/// Type aliases matching `kernel/src/syscall/fs_bridge/special` naming.
pub type TtyRequestWire = TtyRequest;
pub type TtyReplyWire = TtyReply;

const _: () = assert!(core::mem::size_of::<TtyRequest>() == 216);
const _: () = assert!(core::mem::size_of::<TtyReply>() == 200);
const _: () = assert!(core::mem::size_of::<TtyRequest>() <= IPC_KERNEL_MAX_MSG_SIZE);
const _: () = assert!(core::mem::size_of::<TtyReply>() <= IPC_KERNEL_MAX_MSG_SIZE);
