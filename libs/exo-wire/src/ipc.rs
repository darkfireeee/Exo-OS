//! Canonical 200-byte Exo-OS IPC envelope constants, well-known Ring 1 endpoint IDs,
//! and generic `IpcMessage` layout.

pub const IPC_HEADER_SIZE: usize = 8;
pub const IPC_CAP_TOKEN_SIZE: usize = 20;
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 192;
pub const IPC_ENVELOPE_SIZE: usize = IPC_HEADER_SIZE + IPC_INLINE_PAYLOAD_SIZE;
pub const IPC_CAP_TOKEN_PAYLOAD_OFFSET: usize = IPC_INLINE_PAYLOAD_SIZE - IPC_CAP_TOKEN_SIZE;
pub const IPC_CAP_TOKEN_OFFSET: usize = IPC_HEADER_SIZE + IPC_CAP_TOKEN_PAYLOAD_OFFSET;
pub const IPC_KERNEL_MAX_MSG_SIZE: usize = 240;
pub const IPC_RECV_MAX_LEN: usize = 65_536;
pub const IPC_RECV_LEGACY_SHORTHAND_MAX_LEN: usize = IPC_RECV_MAX_LEN;

const _: () = assert!(IPC_INLINE_PAYLOAD_SIZE <= 200);
const _: () = assert!(IPC_ENVELOPE_SIZE == 200);
const _: () = assert!(IPC_ENVELOPE_SIZE <= IPC_KERNEL_MAX_MSG_SIZE);
const _: () = assert!(IPC_CAP_TOKEN_OFFSET + IPC_CAP_TOKEN_SIZE == IPC_ENVELOPE_SIZE);

// Well-known Ring 1 service endpoint IDs
pub const CRYPTO_SERVER_ENDPOINT: u64 = 4;
pub const NETWORK_SERVER_ENDPOINT: u64 = 7;
pub const INPUT_SERVER_ENDPOINT: u64 = 11;
pub const TTY_SERVER_ENDPOINT: u64 = 12;
pub const VIRTIO_DRIVERS_ENDPOINT: u64 = 13;
pub const VIRTIO_NET_ENDPOINT: u64 = 14;
pub const LOOPBACK_DRIVER_ENDPOINT: u64 = 15;
pub const AUDIO_SERVER_ENDPOINT: u64 = 16;
pub const PS2_DRIVER_IRQ_CHANNEL: u64 = 17;
pub const USB_SERVER_ENDPOINT: u64 = 18;
pub const FB_SERVER_ENDPOINT: u64 = 20;
pub const E1000_DRIVER_ENDPOINT: u64 = 21;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IpcMessage {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; IPC_INLINE_PAYLOAD_SIZE],
}

impl IpcMessage {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0u8; IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

impl Default for IpcMessage {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

const _: () = assert!(core::mem::size_of::<IpcMessage>() == IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(IpcMessage, payload) == IPC_HEADER_SIZE);
