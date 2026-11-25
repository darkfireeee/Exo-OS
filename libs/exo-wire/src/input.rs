//! Canonical Input (`ps2`, `usb_hid` ↔ `input_server` ↔ `fb_server`/`tty_server`) wire types.

use crate::ipc::IPC_KERNEL_MAX_MSG_SIZE;

pub const INPUT_MSG_PUSH: u32 = 0x120;
pub const INPUT_MSG_POLL: u32 = 0x121;
pub const INPUT_MSG_HEARTBEAT: u32 = 0x122;
pub const INPUT_MSG_ATTACH: u32 = 0x123;
pub const INPUT_MSG_DETACH: u32 = 0x124;

pub const INPUT_DEVICE_KEYBOARD: u8 = 1;
pub const INPUT_DEVICE_MOUSE: u8 = 2;
pub const INPUT_KEY_RELEASED: u8 = 0;
pub const INPUT_KEY_PRESSED: u8 = 1;
pub const INPUT_MOD_SHIFT: u8 = 1 << 0;
pub const INPUT_MOD_CTRL: u8 = 1 << 1;
pub const INPUT_MOD_ALT: u8 = 1 << 2;
pub const INPUT_MOD_META: u8 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputEventWire {
    pub device: u8,
    pub state: u8,
    pub code: u16,
    pub value: i16,
    pub ascii: u8,
    pub modifiers: u8,
    pub _pad: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InputRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub event: InputEventWire,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InputReply {
    pub status: i64,
    pub event: InputEventWire,
    pub queue_depth: u32,
    pub _pad: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<InputEventWire>() == 12);
const _: () = assert!(core::mem::size_of::<InputRequest>() <= IPC_KERNEL_MAX_MSG_SIZE);
const _: () = assert!(core::mem::size_of::<InputReply>() <= IPC_KERNEL_MAX_MSG_SIZE);
