//! Canonical Audio IPC wire structures (`audio_server`).

use crate::ipc::IPC_KERNEL_MAX_MSG_SIZE;

pub const AUDIO_MSG_PLAY_SYSTEM_SOUND: u32 = 0;
pub const AUDIO_MSG_BEEP: u32 = 1;
pub const AUDIO_MSG_STOP: u32 = 2;

pub const AUDIO_SOUND_BOOT_COMPLETE: u32 = 0;
pub const AUDIO_SOUND_SECURITY_ALERT: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub a: u32,
    pub b: u32,
    pub _reserved: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioReply {
    pub status: i64,
    pub _reserved: [u8; 16],
}

const _: () = assert!(core::mem::size_of::<AudioRequest>() <= IPC_KERNEL_MAX_MSG_SIZE);
const _: () = assert!(core::mem::size_of::<AudioReply>() <= IPC_KERNEL_MAX_MSG_SIZE);
