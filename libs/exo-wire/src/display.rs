//! Canonical Display, Framebuffer, SurfaceGrant (`SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md`),
//! and VirtIO-GPU IPC wire structures.

use crate::ipc::IPC_KERNEL_MAX_MSG_SIZE;

pub const FB_MSG_WRITE: u32 = 0x140;
pub const FB_MSG_CLEAR: u32 = 0x141;
pub const FB_MSG_SCROLL: u32 = 0x142;
pub const FB_MSG_SET_CURSOR: u32 = 0x143;
pub const FB_TEXT_MAX: usize = 208;

pub const DISPLAY_OP_GET_INFO: u32 = 0x5001;
pub const DISPLAY_OP_CREATE_SURFACE: u32 = 0x5002;
pub const DISPLAY_OP_DESTROY_SURFACE: u32 = 0x5003;
pub const DISPLAY_OP_COMMIT_DAMAGE: u32 = 0x5004;
pub const DISPLAY_OP_FLIP: u32 = 0x5005;
pub const DISPLAY_OP_MOVE_WINDOW: u32 = 0x5006;
pub const DISPLAY_OP_SET_BRIGHTNESS: u32 = 0x5007;
pub const DISPLAY_OP_WAYLAND_WIRE: u32 = 0x5010;

/// Flag set in bit 63 of `FbRequest::a` on `DISPLAY_OP_CREATE_SURFACE` to request a
/// fullscreen borderless root desktop compositor surface (`exo_compositor` Strate 2 -> `fb_server`).
pub const DISPLAY_SURFACE_FLAG_ROOT_DESKTOP: u64 = 1u64 << 63;

pub const VIRTIO_GPU_MSG_TRANSFER_FLUSH: u32 = 2;
pub const VIRTIO_GPU_MSG_MOVE_CURSOR: u32 = 3;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Unknown = 0,
    Rgbx8888 = 1,
    Bgrx8888 = 2,
    Rgb888 = 3,
    Bgr888 = 4,
    Rgb565 = 5,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

const _: () = assert!(core::mem::size_of::<DamageRect>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SurfaceGrant {
    pub surface_id: u32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub format: u32,
    pub flags: u32,
    pub virt_addr: u64,
}

const _: () = assert!(core::mem::size_of::<SurfaceGrant>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FramebufferInfoWire {
    pub phys_addr: u64,
    pub width: u32,
    pub height: u32,
    pub stride_pixels: u32,
    pub bpp: u32,
    pub format: u32,
    pub _pad: u32,
    pub size_bytes: u64,
}

const _: () = assert!(core::mem::size_of::<FramebufferInfoWire>() == 40);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub reply_endpoint: u64,
    pub a: u64,
    pub b: u64,
    pub data: [u8; FB_TEXT_MAX],
}

impl FbRequest {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            reply_endpoint: 0,
            a: 0,
            b: 0,
            data: [0; FB_TEXT_MAX],
        }
    }
}

const _: () = assert!(core::mem::size_of::<FbRequest>() <= IPC_KERNEL_MAX_MSG_SIZE);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbReply {
    pub status: i64,
    pub len: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<FbReply>() == 16);
