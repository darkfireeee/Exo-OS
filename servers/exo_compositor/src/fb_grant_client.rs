//! Typed `SurfaceGrant` & `DamageRect` IPC client toward Strate 1 `fb_server`.

use exo_syscall_abi as syscall;

#[allow(dead_code)]
pub struct FbSurfaceGrant {
    pub surface_id: u32,
    pub shm_virt: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub size_bytes: u64,
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

pub fn request_fb_surface_grant(pid: u32, reply_ep: u64) -> Option<FbSurfaceGrant> {
    let mut info_req = syscall::FbRequest::zeroed();
    info_req.sender_pid = pid;
    info_req.msg_type = syscall::DISPLAY_OP_GET_INFO;
    info_req.reply_endpoint = reply_ep;
    let mut info_reply = syscall::FbReply::default();
    let irc = unsafe {
        syscall::syscall6(
            syscall::SYS_EXO_IPC_CALL,
            syscall::FB_SERVER_ENDPOINT,
            &info_req as *const syscall::FbRequest as u64,
            core::mem::size_of::<syscall::FbRequest>() as u64,
            &mut info_reply as *mut syscall::FbReply as u64,
            core::mem::size_of::<syscall::FbReply>() as u64,
            0,
        )
    };
    if irc < 0 || info_reply.status <= 0 {
        return None;
    }
    let fb_w = ((info_reply.status as u64) >> 32) as u32;
    let fb_h = (info_reply.status as u64 & 0xFFFF_FFFF) as u32;
    let w = fb_w.clamp(64, 1280);
    let h = fb_h.clamp(64, 720);

    let mut create_req = syscall::FbRequest::zeroed();
    create_req.sender_pid = pid;
    create_req.msg_type = syscall::DISPLAY_OP_CREATE_SURFACE;
    create_req.reply_endpoint = reply_ep;
    create_req.a = w as u64;
    create_req.b = h as u64;
    let mut create_reply = syscall::FbReply::default();
    let crc = unsafe {
        syscall::syscall6(
            syscall::SYS_EXO_IPC_CALL,
            syscall::FB_SERVER_ENDPOINT,
            &create_req as *const syscall::FbRequest as u64,
            core::mem::size_of::<syscall::FbRequest>() as u64,
            &mut create_reply as *mut syscall::FbReply as u64,
            core::mem::size_of::<syscall::FbReply>() as u64,
            0,
        )
    };
    if crc < 0 || create_reply.status <= 0 || create_reply.len == 0 {
        return None;
    }
    let stride = create_reply._pad.max(w.saturating_mul(4));
    let size_bytes = (stride as u64).saturating_mul(h as u64);
    Some(FbSurfaceGrant {
        surface_id: create_reply.len,
        shm_virt: 0, // Populated via SurfaceGrant handle when mapped
        width: w,
        height: h,
        stride,
        size_bytes,
    })
}

pub fn submit_damage_to_fb_server(pid: u32, grant: &FbSurfaceGrant, dmg: syscall::DamageRect) {
    let mut req = syscall::FbRequest::zeroed();
    req.sender_pid = pid;
    req.msg_type = syscall::DISPLAY_OP_COMMIT_DAMAGE;
    req.reply_endpoint = 0;
    let x = dmg.x.min(0xFFFF) as u64;
    let y = dmg.y.min(0xFFFF) as u64;
    let w = dmg.width.min(grant.width).min(0xFFFF) as u64;
    let h = dmg.height.min(grant.height).min(0xFFFF) as u64;
    req.a = ((grant.surface_id as u64) << 32) | (y << 16) | x;
    req.b = (h << 16) | w;
    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::FB_SERVER_ENDPOINT,
            &req as *const syscall::FbRequest as u64,
            core::mem::size_of::<syscall::FbRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | 8,
            0,
            0,
        );
    }
}
