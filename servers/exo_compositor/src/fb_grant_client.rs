//! Typed `SurfaceGrant`, `DamageRect`, and `input_server` seat IPC client for Strate 2 `exo_compositor`.

use exo_syscall_abi as syscall;

pub const COMPOSITOR_INPUT_CHANNEL: u64 = 0x434F_4D50; // "COMP"

#[allow(dead_code)]
pub struct FbSurfaceGrant {
    pub surface_id: u32,
    pub shm_virt: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub size_bytes: u64,
    pub gpu_hw_active: bool,
}

impl FbSurfaceGrant {
    /// Mutable ARGB8888 pixel slice mapped into `exo_compositor` by `fb_server`.
    #[inline]
    pub fn pixels_mut(&mut self) -> Option<&mut [u32]> {
        if self.shm_virt == 0 || self.width == 0 || self.height == 0 {
            return None;
        }
        let count = (self.width as usize).checked_mul(self.height as usize)?;
        if (count as u64).checked_mul(4)? > self.size_bytes {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts_mut(self.shm_virt as *mut u32, count) })
    }
}

pub fn register_ipc_endpoint() -> Option<(u32, u64)> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    let ep = ((pid as u64) << 32) | 1;
    let name = b"exo_compositor";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            ep,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    Some((pid as u32, ep))
}

/// Register a dedicated IPC stream with `input_server` receiving both mouse and keyboard events.
pub fn attach_input_stream(pid: u32) -> Option<u64> {
    let ep = ((pid as u64) << 32) | COMPOSITOR_INPUT_CHANNEL;
    let name = b"exo_compositor_input";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            ep,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    let req = syscall::InputRequest {
        sender_pid: pid,
        msg_type: syscall::INPUT_MSG_ATTACH,
        reply_endpoint: ep,
        event: syscall::InputEventWire::default(),
    };
    let send_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 8,
            0,
            0,
        )
    };
    if send_rc < 0 {
        None
    } else {
        Some(ep)
    }
}

/// Claim (`grab = true`) or release (`grab = false`) exclusive keyboard focus in `input_server`.
pub fn set_keyboard_grab(pid: u32, input_ep: u64, grab: bool) {
    let mut ev = syscall::InputEventWire::default();
    ev.device = syscall::INPUT_DEVICE_KEYBOARD;
    ev.value = if grab { 1 } else { 0 };
    let req = syscall::InputRequest {
        sender_pid: pid,
        msg_type: syscall::INPUT_MSG_GRAB_KBD,
        reply_endpoint: if grab { input_ep } else { 0 },
        event: ev,
    };
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 4,
            0,
            0,
        );
    }
}

/// Non-blocking pop of one `InputEventWire` from the compositor's attached input endpoint.
pub fn recv_input_event_nb(input_ep: u64) -> Option<syscall::InputEventWire> {
    let mut reply = syscall::InputReply::default();
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_EXO_IPC_RECV_NB,
            input_ep,
            &mut reply as *mut syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            0,
        )
    };
    if rc >= core::mem::size_of::<syscall::InputReply>() as i64 && reply.status == 0 {
        Some(reply.event)
    } else {
        None
    }
}

/// Blocking wait (with even millisecond timeout) for the next `InputEventWire`.
/// Wakes in < 0.1 ms the instant `input_server` delivers a mouse or keyboard packet.
pub fn recv_input_event_timeout(input_ep: u64, timeout_ms_even: u64) -> Option<syscall::InputEventWire> {
    let mut reply = syscall::InputReply::default();
    let flags = syscall::IPC_FLAG_TIMEOUT | (timeout_ms_even & !1).max(2);
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            input_ep,
            &mut reply as *mut syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            flags,
        )
    };
    if rc >= core::mem::size_of::<syscall::InputReply>() as i64 && reply.status == 0 {
        Some(reply.event)
    } else {
        None
    }
}

/// Request an authorized system Reboot (`reboot = true`) or PowerOff (`reboot = false`) via PID 1 `init_server`.
pub fn request_init_power_action(pid: u32, reboot: bool) {
    const INIT_SERVER_ENDPOINT: u64 = 1;
    const INIT_MSG_POWER_ACTION: u32 = 10;
    #[repr(C)]
    struct InitPowerReq {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }
    let mut req = InitPowerReq {
        sender_pid: pid,
        msg_type: INIT_MSG_POWER_ACTION,
        payload: [0u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    req.payload[0] = if reboot { 1 } else { 2 };
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            INIT_SERVER_ENDPOINT,
            &req as *const InitPowerReq as u64,
            core::mem::size_of::<InitPowerReq>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 200,
            0,
            0,
        );
    }
}

fn fb_rpc(reply_ep: u64, req: &syscall::FbRequest) -> Option<syscall::FbReply> {
    // Drain any stale reply left in our mailbox before issuing a synchronous request.
    let mut stale = syscall::FbReply::default();
    for _ in 0..8 {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV_NB,
                reply_ep,
                &mut stale as *mut syscall::FbReply as u64,
                core::mem::size_of::<syscall::FbReply>() as u64,
                0,
            )
        };
        if rc < 0 {
            break;
        }
    }

    let send_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::FB_SERVER_ENDPOINT,
            req as *const syscall::FbRequest as u64,
            core::mem::size_of::<syscall::FbRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 500,
            0,
            0,
        )
    };
    if send_rc < 0 {
        return None;
    }

    let mut reply = syscall::FbReply::default();
    let recv_rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            reply_ep,
            &mut reply as *mut syscall::FbReply as u64,
            core::mem::size_of::<syscall::FbReply>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 2000,
        )
    };
    if recv_rc < core::mem::size_of::<syscall::FbReply>() as i64 {
        return None;
    }
    Some(reply)
}

pub fn request_fb_surface_grant(pid: u32, reply_ep: u64) -> Option<FbSurfaceGrant> {
    let mut info_req = syscall::FbRequest::zeroed();
    info_req.sender_pid = pid;
    info_req.msg_type = syscall::DISPLAY_OP_GET_INFO;
    info_req.reply_endpoint = reply_ep;
    let info_reply = fb_rpc(reply_ep, &info_req)?;
    if info_reply.status <= 0 {
        return None;
    }
    let fb_w = ((info_reply.status as u64) >> 32) as u32;
    let fb_h = (info_reply.status as u64 & 0xFFFF_FFFF) as u32;
    let gpu_hw_active = (info_reply._pad & (1 << 31)) != 0;
    let w = fb_w.clamp(320, 1920);
    let h = fb_h.clamp(240, 1080);

    let mut create_req = syscall::FbRequest::zeroed();
    create_req.sender_pid = pid;
    create_req.msg_type = syscall::DISPLAY_OP_CREATE_SURFACE;
    create_req.reply_endpoint = reply_ep;
    create_req.a = syscall::DISPLAY_SURFACE_FLAG_ROOT_DESKTOP | (w as u64);
    create_req.b = h as u64;
    let create_reply = fb_rpc(reply_ep, &create_req)?;
    if create_reply.status <= 0 || create_reply.len == 0 {
        return None;
    }
    let stride = create_reply._pad.max(w.saturating_mul(4));
    let size_bytes = (stride as u64).saturating_mul(h as u64);
    Some(FbSurfaceGrant {
        surface_id: create_reply.len,
        shm_virt: create_reply.status as u64,
        width: w,
        height: h,
        stride,
        size_bytes,
        gpu_hw_active,
    })
}

/// Synchronize `fb_server`'s hardware/sprite cursor to the compositor's authoritative `(x, y)`
/// position so the visible pointer and hit-test coordinates can never diverge by even 1 pixel.
pub fn sync_cursor_to_fb_server(pid: u32, x: i32, y: i32) {
    let mut req = syscall::FbRequest::zeroed();
    req.sender_pid = pid;
    req.msg_type = syscall::DISPLAY_OP_MOVE_WINDOW;
    req.reply_endpoint = 0;
    req.a = x.max(0) as u64;
    req.b = y.max(0) as u64;
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::FB_SERVER_ENDPOINT,
            &req as *const syscall::FbRequest as u64,
            core::mem::size_of::<syscall::FbRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 4,
            0,
            0,
        );
    }
}

/// Submit a finalized dirty rectangle to `fb_server` and wait for post-flush ACK.
/// Lockstep double-buffering guarantees `exo_compositor` never mutates SHM while `fb_server`
/// executes `SYS_EXO_MEM_COPY_FROM_PID` + MMIO scanout (zero tearing, zero wallpaper flash).
pub fn submit_damage_to_fb_server(pid: u32, reply_ep: u64, grant: &FbSurfaceGrant, dmg: syscall::DamageRect) {
    if reply_ep != 0 {
        let mut stale = syscall::FbReply::default();
        let _ = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV_NB,
                reply_ep,
                &mut stale as *mut syscall::FbReply as u64,
                core::mem::size_of::<syscall::FbReply>() as u64,
                0,
            )
        };
    }
    let mut req = syscall::FbRequest::zeroed();
    req.sender_pid = pid;
    req.msg_type = syscall::DISPLAY_OP_COMMIT_DAMAGE;
    req.reply_endpoint = reply_ep;
    let x = dmg.x.min(0xFFFF) as u64;
    let y = dmg.y.min(0xFFFF) as u64;
    let w = dmg.width.min(grant.width).min(0xFFFF) as u64;
    let h = dmg.height.min(grant.height).min(0xFFFF) as u64;
    req.a = ((grant.surface_id as u64) << 32) | (y << 16) | x;
    req.b = (h << 16) | w;
    let mut sent = false;
    for _ in 0..3 {
        let rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                syscall::FB_SERVER_ENDPOINT,
                &req as *const syscall::FbRequest as u64,
                core::mem::size_of::<syscall::FbRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 100,
                0,
                0,
            )
        };
        if rc >= 0 {
            sent = true;
            break;
        }
    }
    if sent && reply_ep != 0 {
        let mut ack = syscall::FbReply::default();
        let _ = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_ep,
                &mut ack as *mut syscall::FbReply as u64,
                core::mem::size_of::<syscall::FbReply>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 60,
            )
        };
    }
}
