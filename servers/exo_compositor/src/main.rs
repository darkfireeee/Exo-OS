//! # `exo_compositor` — Isolated Strate 2 Wayland Compositor (S5B)
//!
//! Zero-Trust architectural split from `fb_server`:
//! - Runs in Rust `std` (`x86_64-unknown-exo`) with **ZERO MMIO** and **ZERO hardware privileges**.
//! - Hosts `/run/wayland-0` (`AF_UNIX` + `SCM_RIGHTS` + `epoll`).
//! - Enforces `F_SEAL_SHRINK`, `fstat` backing size validation, checked arithmetic
//!   `(offset + height * stride) <= pool_size`, and strict per-client quotas.
//! - Submits composited damage strictly via typed `SurfaceGrant` + `DamageRect` IPC
//!   messages to `fb_server` over `ExoCordon` (`ServiceClass::ExoCompositor -> FbServer`).
//! - Belongs to **ExoPhoenix Strate 2** (transient, restartable by `init_server` without
//!   impacting `fb_server` or the 1 s SLA).

use exo_syscall_abi as syscall;
use std::collections::{HashMap, VecDeque};

pub const MAX_WAYLAND_CLIENTS: usize = 8;
pub const MAX_OBJECTS_PER_CLIENT: usize = 32;
pub const MAX_POOLS_PER_CLIENT: usize = 4;
pub const MAX_SHM_BYTES_PER_CLIENT: u64 = 16 * 1024 * 1024;
pub const MAX_WIRE_MSG_SIZE: usize = 4096;
pub const MAX_RX_BUF: usize = 8192;

const AF_UNIX: u64 = 1;
const SOCK_STREAM: u64 = 1;
const MSG_DONTWAIT: u64 = 0x40;
const EINTR_ERR: i64 = -4;
const F_ADD_SEALS: u64 = 1033;
const F_GET_SEALS: u64 = 1034;
const F_SEAL_SHRINK: u64 = 0x0002;
const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLLIN: u32 = 0x0001;

#[repr(C)]
struct SockAddrUn {
    sun_family: u16,
    sun_path: [u8; 108],
}

#[repr(C)]
struct RawIoVec {
    iov_base: u64,
    iov_len: u64,
}

#[repr(C)]
struct UnixMsghdr {
    msg_name: u64,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: i32,
    _pad1: u32,
}

#[repr(C)]
struct UnixCmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ObjKind {
    Display,
    Registry,
    Compositor,
    Shm,
    ShmPool,
    Buffer,
    Surface,
    XdgWmBase,
    XdgSurface,
    XdgToplevel,
    Seat,
    Pointer,
    Keyboard,
    Region,
}

#[derive(Clone, Copy, Debug)]
struct ShmPool {
    fd: i32,
    size: u32,
    local_addr: u64,
    mapped_len: u64,
}

#[derive(Clone, Copy, Debug)]
struct WlBuffer {
    pool_id: u32,
    offset: u32,
    width: u32,
    height: u32,
    stride: u32,
    bpp: u32,
}

#[derive(Clone, Debug, Default)]
struct WlSurface {
    attached_buffer: u32,
    xdg_surface_id: u32,
    xdg_toplevel_id: u32,
    frame_callback_id: u32,
    configure_sent: bool,
    toplevel: bool,
    damage: syscall::DamageRect,
}

struct ClientState {
    fd: i32,
    rx_buf: Vec<u8>,
    tx_buf: Vec<u8>,
    rx_fds: VecDeque<i32>,
    objects: HashMap<u32, ObjKind>,
    pools: HashMap<u32, ShmPool>,
    buffers: HashMap<u32, WlBuffer>,
    surfaces: HashMap<u32, WlSurface>,
    xdg_to_surface: HashMap<u32, u32>,
    next_serial: u32,
}

impl ClientState {
    fn new(fd: i32) -> Self {
        let mut objects = HashMap::new();
        objects.insert(1, ObjKind::Display);
        Self {
            fd,
            rx_buf: Vec::with_capacity(2048),
            tx_buf: Vec::with_capacity(2048),
            rx_fds: VecDeque::new(),
            objects,
            pools: HashMap::new(),
            buffers: HashMap::new(),
            surfaces: HashMap::new(),
            xdg_to_surface: HashMap::new(),
            next_serial: 1,
        }
    }

    fn total_mapped_shm(&self) -> u64 {
        self.pools
            .values()
            .fold(0u64, |acc, p| acc.saturating_add(p.mapped_len))
    }

    fn register_obj(&mut self, id: u32, kind: ObjKind) -> bool {
        if id == 0 {
            return false;
        }
        if !self.objects.contains_key(&id) && self.objects.len() >= MAX_OBJECTS_PER_CLIENT {
            return false;
        }
        self.objects.insert(id, kind);
        true
    }

    fn next_serial(&mut self) -> u32 {
        let s = self.next_serial.max(1);
        self.next_serial = s.wrapping_add(1).max(1);
        s
    }

    fn cleanup(&mut self) {
        for (_, pool) in self.pools.drain() {
            unmap_close_pool(pool);
        }
        while let Some(rfd) = self.rx_fds.pop_front() {
            if rfd >= 0 {
                unsafe {
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, rfd as u64);
                }
            }
        }
        if self.fd >= 0 {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, self.fd as u64);
            }
            self.fd = -1;
        }
    }
}

impl Drop for ClientState {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[allow(dead_code)]
struct FbSurfaceGrant {
    surface_id: u32,
    shm_virt: u64,
    width: u32,
    height: u32,
    stride: u32,
    size_bytes: u64,
}

fn boot_log(msg: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(syscall::SYS_EXO_LOG, msg.as_ptr() as u64, msg.len() as u64, 1);
    }
}

#[inline]
fn read_u32(bytes: &[u8], off: usize) -> u32 {
    if off + 4 > bytes.len() {
        return 0;
    }
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

fn parse_wl_str(bytes: &[u8]) -> &[u8] {
    if bytes.len() < 4 {
        return &[];
    }
    let len = read_u32(bytes, 0) as usize;
    if len == 0 || 4 + len > bytes.len() {
        return &[];
    }
    let mut s = &bytes[4..4 + len];
    while let Some(&b) = s.last() {
        if b == 0 {
            s = &s[..s.len() - 1];
        } else {
            break;
        }
    }
    s
}

#[inline]
pub fn validate_shm_buffer_bounds(
    offset: u32,
    width: u32,
    height: u32,
    stride: u32,
    bpp: u32,
    pool_limit: u64,
) -> bool {
    if width == 0 || height == 0 || width > 4096 || height > 4096 || bpp == 0 {
        return false;
    }
    let Some(min_stride) = width.checked_mul(bpp) else {
        return false;
    };
    if stride < min_stride {
        return false;
    }
    let Some(total_rows) = (height as u64).checked_mul(stride as u64) else {
        return false;
    };
    let Some(end_off) = (offset as u64).checked_add(total_rows) else {
        return false;
    };
    end_off <= pool_limit
}

fn format_bpp(fmt: u32) -> u32 {
    match fmt {
        4 | 0x36314752 => 2,
        5 | 0x34324752 | 6 | 0x34324742 => 3,
        _ => 4,
    }
}

fn push_empty_evt(tx: &mut Vec<u8>, obj: u32, opcode: u16) {
    let hdr = (8u32 << 16) | (opcode as u32);
    tx.extend_from_slice(&obj.to_le_bytes());
    tx.extend_from_slice(&hdr.to_le_bytes());
}

fn push_u32_evt(tx: &mut Vec<u8>, obj: u32, opcode: u16, v: u32) {
    let hdr = (12u32 << 16) | (opcode as u32);
    tx.extend_from_slice(&obj.to_le_bytes());
    tx.extend_from_slice(&hdr.to_le_bytes());
    tx.extend_from_slice(&v.to_le_bytes());
}

fn push_wl_str_bytes(tx: &mut Vec<u8>, s: &[u8]) {
    let slen = s.len() + 1;
    let padded = (slen + 3) & !3;
    tx.extend_from_slice(&(slen as u32).to_le_bytes());
    tx.extend_from_slice(s);
    tx.resize(tx.len() + (padded - s.len()), 0);
}

fn push_str_evt(tx: &mut Vec<u8>, obj: u32, opcode: u16, s: &[u8]) {
    let padded = ((s.len() + 1) + 3) & !3;
    let total = 8 + 4 + padded;
    let hdr = ((total as u32) << 16) | (opcode as u32);
    tx.extend_from_slice(&obj.to_le_bytes());
    tx.extend_from_slice(&hdr.to_le_bytes());
    push_wl_str_bytes(tx, s);
}

fn push_registry_global(tx: &mut Vec<u8>, reg_id: u32, name: u32, iface: &[u8], version: u32) {
    let padded = ((iface.len() + 1) + 3) & !3;
    let total = 8 + 4 + 4 + padded + 4;
    let hdr = (total as u32) << 16;
    tx.extend_from_slice(&reg_id.to_le_bytes());
    tx.extend_from_slice(&hdr.to_le_bytes());
    tx.extend_from_slice(&name.to_le_bytes());
    push_wl_str_bytes(tx, iface);
    tx.extend_from_slice(&version.to_le_bytes());
}

fn push_xdg_toplevel_configure(tx: &mut Vec<u8>, obj: u32, width: i32, height: i32) {
    let hdr = (20u32 << 16) | 0u32;
    tx.extend_from_slice(&obj.to_le_bytes());
    tx.extend_from_slice(&hdr.to_le_bytes());
    tx.extend_from_slice(&width.to_le_bytes());
    tx.extend_from_slice(&height.to_le_bytes());
    tx.extend_from_slice(&0u32.to_le_bytes()); // empty states array
}

fn unmap_close_pool(pool: ShmPool) {
    if pool.local_addr != 0 && pool.mapped_len != 0 {
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_MUNMAP, pool.local_addr, pool.mapped_len);
        }
    }
    if pool.fd >= 0 {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, pool.fd as u64);
        }
    }
}

fn map_verified_shm_fd(fd: i32, size: u32) -> Option<(u64, u64)> {
    if fd < 0 || size == 0 || (size as u64) > MAX_SHM_BYTES_PER_CLIENT {
        return None;
    }
    // Verify backing file size via `fstat` (st_size at offset 48 in 144-byte LinuxStat).
    let mut stat_buf = [0u8; 144];
    let st_rc = unsafe { syscall::syscall2(syscall::SYS_FSTAT, fd as u64, stat_buf.as_mut_ptr() as u64) };
    if st_rc < 0 {
        return None;
    }
    let st_size = i64::from_le_bytes([
        stat_buf[48],
        stat_buf[49],
        stat_buf[50],
        stat_buf[51],
        stat_buf[52],
        stat_buf[53],
        stat_buf[54],
        stat_buf[55],
    ]);
    if st_size <= 0 || (size as i64) > st_size {
        return None;
    }
    // Enforce `F_SEAL_SHRINK` on `memfd` SHM pools before mapping.
    let seals = unsafe { syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_GET_SEALS, 0) };
    if seals >= 0 && ((seals as u64) & F_SEAL_SHRINK) == 0 {
        let add_rc = unsafe {
            syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_ADD_SEALS, F_SEAL_SHRINK)
        };
        if add_rc < 0 {
            let seals2 = unsafe { syscall::syscall3(syscall::SYS_FCNTL, fd as u64, F_GET_SEALS, 0) };
            if seals2 < 0 || ((seals2 as u64) & F_SEAL_SHRINK) == 0 {
                return None;
            }
        }
    }
    let aligned = ((size as u64).max(4096) + 4095) & !4095;
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            0,
            aligned,
            syscall::PROT_READ,
            syscall::MAP_SHARED,
            fd as u64,
            0,
        )
    };
    if rc > 0 {
        Some((rc as u64, aligned))
    } else {
        None
    }
}

fn register_ipc_endpoint() -> Option<(u32, u64)> {
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

fn request_fb_surface_grant(pid: u32, reply_ep: u64) -> Option<FbSurfaceGrant> {
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

fn submit_damage_to_fb_server(pid: u32, grant: &FbSurfaceGrant, dmg: syscall::DamageRect) {
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

fn dispatch_client_messages(
    client: &mut ClientState,
    pid: u32,
    fb_grant: Option<&FbSurfaceGrant>,
) -> bool {
    let mut pos = 0usize;
    while pos + 8 <= client.rx_buf.len() {
        let obj_id = read_u32(&client.rx_buf, pos);
        let size_op = read_u32(&client.rx_buf, pos + 4);
        let raw_size = (size_op >> 16) as usize;
        let opcode = (size_op & 0xFFFF) as u16;
        if raw_size < 8 || (raw_size & 3) != 0 || raw_size > MAX_WIRE_MSG_SIZE {
            return false;
        }
        if pos + raw_size > client.rx_buf.len() {
            break;
        }
        let payload = client.rx_buf[pos + 8..pos + raw_size].to_vec();
        pos += raw_size;

        let Some(&kind) = client.objects.get(&obj_id) else {
            return false;
        };

        match (kind, opcode) {
            // wl_display.sync(new_id)
            (ObjKind::Display, 0) => {
                let cb_id = read_u32(&payload, 0);
                if cb_id == 0 {
                    return false;
                }
                push_u32_evt(&mut client.tx_buf, cb_id, 0, 0);
                push_u32_evt(&mut client.tx_buf, 1, 1, cb_id);
            }
            // wl_display.get_registry(new_id)
            (ObjKind::Display, 1) => {
                let reg_id = read_u32(&payload, 0);
                if !client.register_obj(reg_id, ObjKind::Registry) {
                    return false;
                }
                push_registry_global(&mut client.tx_buf, reg_id, 1, b"wl_compositor", 6);
                push_registry_global(&mut client.tx_buf, reg_id, 2, b"wl_shm", 1);
                push_registry_global(&mut client.tx_buf, reg_id, 3, b"xdg_wm_base", 6);
                push_registry_global(&mut client.tx_buf, reg_id, 4, b"wl_seat", 7);
            }
            // wl_registry.bind(name, [iface, ver], new_id)
            (ObjKind::Registry, 0) => {
                if payload.len() < 8 {
                    return false;
                }
                let name = read_u32(&payload, 0);
                let new_id = read_u32(&payload, payload.len() - 4);
                let iface = if payload.len() >= 16 {
                    parse_wl_str(&payload[4..])
                } else {
                    &[]
                };
                let target_kind = if iface == b"wl_compositor" {
                    ObjKind::Compositor
                } else if iface == b"wl_shm" {
                    ObjKind::Shm
                } else if iface == b"xdg_wm_base" {
                    ObjKind::XdgWmBase
                } else if iface == b"wl_seat" {
                    ObjKind::Seat
                } else {
                    match name {
                        1 => ObjKind::Compositor,
                        2 => ObjKind::Shm,
                        3 => ObjKind::XdgWmBase,
                        4 => ObjKind::Seat,
                        _ => return false,
                    }
                };
                if !client.register_obj(new_id, target_kind) {
                    return false;
                }
                match target_kind {
                    ObjKind::Shm => {
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 0); // ARGB8888
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 1); // XRGB8888
                    }
                    ObjKind::Seat => {
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 3); // pointer | keyboard
                        push_str_evt(&mut client.tx_buf, new_id, 1, b"seat0");
                    }
                    _ => {}
                }
            }
            // wl_compositor.create_surface
            (ObjKind::Compositor, 0) => {
                let sid = read_u32(&payload, 0);
                if !client.register_obj(sid, ObjKind::Surface) {
                    return false;
                }
                client.surfaces.insert(sid, WlSurface::default());
            }
            // wl_compositor.create_region
            (ObjKind::Compositor, 1) => {
                let rid = read_u32(&payload, 0);
                if !client.register_obj(rid, ObjKind::Region) {
                    return false;
                }
            }
            // wl_compositor.release / wl_region.destroy / wl_shm.release / xdg_wm_base.destroy / wl_seat.release
            (ObjKind::Compositor, 2)
            | (ObjKind::Region, 0)
            | (ObjKind::Shm, 1)
            | (ObjKind::XdgWmBase, 0)
            | (ObjKind::Seat, 3) => {
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // wl_shm.create_pool(new_id pool, size) + SCM_RIGHTS fd
            (ObjKind::Shm, 0) => {
                let pool_id = read_u32(&payload, 0);
                let fd = client.rx_fds.pop_front().unwrap_or(-1);
                let size = if payload.len() >= 12 {
                    read_u32(&payload, 8)
                } else {
                    read_u32(&payload, 4)
                };
                let size_aligned = (size as u64)
                    .checked_add(4095)
                    .map(|v| v & !4095)
                    .unwrap_or(u64::MAX);
                if pool_id == 0
                    || fd < 0
                    || size == 0
                    || client.pools.len() >= MAX_POOLS_PER_CLIENT
                    || client.total_mapped_shm().saturating_add(size_aligned)
                        > MAX_SHM_BYTES_PER_CLIENT
                {
                    if fd >= 0 {
                        unsafe {
                            let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                        }
                    }
                    return false;
                }
                let Some((local_addr, mapped_len)) = map_verified_shm_fd(fd, size) else {
                    unsafe {
                        let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                    }
                    return false;
                };
                if !client.register_obj(pool_id, ObjKind::ShmPool) {
                    unmap_close_pool(ShmPool {
                        fd,
                        size,
                        local_addr,
                        mapped_len,
                    });
                    return false;
                }
                if let Some(old) = client.pools.insert(
                    pool_id,
                    ShmPool {
                        fd,
                        size,
                        local_addr,
                        mapped_len,
                    },
                ) {
                    unmap_close_pool(old);
                }
            }
            // wl_shm_pool.create_buffer(new_id, offset, width, height, stride, format)
            (ObjKind::ShmPool, 0) => {
                if payload.len() < 24 {
                    return false;
                }
                let buf_id = read_u32(&payload, 0);
                let offset = read_u32(&payload, 4);
                let width = read_u32(&payload, 8);
                let height = read_u32(&payload, 12);
                let stride = read_u32(&payload, 16);
                let format = read_u32(&payload, 20);
                let bpp = format_bpp(format);
                let Some(pool) = client.pools.get(&obj_id).copied() else {
                    return false;
                };
                let limit = (pool.size as u64).min(pool.mapped_len);
                if !validate_shm_buffer_bounds(offset, width, height, stride, bpp, limit) {
                    return false;
                }
                if !client.register_obj(buf_id, ObjKind::Buffer) {
                    return false;
                }
                client.buffers.insert(
                    buf_id,
                    WlBuffer {
                        pool_id: obj_id,
                        offset,
                        width,
                        height,
                        stride,
                        bpp,
                    },
                );
            }
            // wl_shm_pool.destroy()
            (ObjKind::ShmPool, 1) => {
                if let Some(pool) = client.pools.remove(&obj_id) {
                    unmap_close_pool(pool);
                }
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // wl_shm_pool.resize(size)
            (ObjKind::ShmPool, 2) => {
                let new_sz = read_u32(&payload, 0);
                let Some(old_pool) = client.pools.get(&obj_id).copied() else {
                    return false;
                };
                let new_aligned = (new_sz as u64)
                    .checked_add(4095)
                    .map(|v| v & !4095)
                    .unwrap_or(u64::MAX);
                let delta = new_aligned.saturating_sub(old_pool.mapped_len);
                if new_sz < old_pool.size
                    || (new_sz as u64) > MAX_SHM_BYTES_PER_CLIENT
                    || client.total_mapped_shm().saturating_add(delta) > MAX_SHM_BYTES_PER_CLIENT
                {
                    return false;
                }
                if (new_sz as u64) > old_pool.mapped_len {
                    let Some((new_addr, new_len)) = map_verified_shm_fd(old_pool.fd, new_sz) else {
                        return false;
                    };
                    if old_pool.local_addr != 0 && old_pool.mapped_len != 0 {
                        unsafe {
                            let _ = syscall::syscall2(
                                syscall::SYS_MUNMAP,
                                old_pool.local_addr,
                                old_pool.mapped_len,
                            );
                        }
                    }
                    if let Some(p) = client.pools.get_mut(&obj_id) {
                        p.local_addr = new_addr;
                        p.mapped_len = new_len;
                        p.size = new_sz;
                    }
                } else if let Some(p) = client.pools.get_mut(&obj_id) {
                    p.size = new_sz;
                }
            }
            // wl_buffer.destroy()
            (ObjKind::Buffer, 0) => {
                client.buffers.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // wl_surface.destroy()
            (ObjKind::Surface, 0) => {
                client.surfaces.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // wl_surface.attach(buffer_id, x, y)
            (ObjKind::Surface, 1) => {
                let buf_id = read_u32(&payload, 0);
                if buf_id != 0 && !client.buffers.contains_key(&buf_id) {
                    return false;
                }
                if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                    surf.attached_buffer = buf_id;
                }
            }
            // wl_surface.damage / damage_buffer(x, y, w, h)
            (ObjKind::Surface, 2 | 9) => {
                if payload.len() >= 16 {
                    if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                        surf.damage = syscall::DamageRect {
                            x: read_u32(&payload, 0),
                            y: read_u32(&payload, 4),
                            width: read_u32(&payload, 8),
                            height: read_u32(&payload, 12),
                        };
                    }
                }
            }
            // wl_surface.frame(new_id)
            (ObjKind::Surface, 3) => {
                let cb_id = read_u32(&payload, 0);
                if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                    surf.frame_callback_id = cb_id;
                }
            }
            // wl_surface.commit()
            (ObjKind::Surface, 6) => {
                let mut attached_buf = 0u32;
                let mut need_configure = false;
                let mut xdg_surf_id = 0u32;
                let mut xdg_top_id = 0u32;
                let mut frame_cb = 0u32;
                let mut damage = syscall::DamageRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                };
                if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                    attached_buf = surf.attached_buffer;
                    need_configure = surf.toplevel && surf.xdg_surface_id != 0 && !surf.configure_sent;
                    xdg_surf_id = surf.xdg_surface_id;
                    xdg_top_id = surf.xdg_toplevel_id;
                    frame_cb = surf.frame_callback_id;
                    surf.frame_callback_id = 0;
                    damage = surf.damage;
                    surf.damage = syscall::DamageRect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    };
                    if need_configure {
                        surf.configure_sent = true;
                    }
                }
                if attached_buf != 0 {
                    // Re-verify buffer bounds safely before touching pixels or forwarding damage.
                    if let Some(buf) = client.buffers.get(&attached_buf).copied() {
                        if let Some(pool) = client.pools.get(&buf.pool_id).copied() {
                            let limit = (pool.size as u64).min(pool.mapped_len);
                            if !validate_shm_buffer_bounds(
                                buf.offset, buf.width, buf.height, buf.stride, buf.bpp, limit,
                            ) {
                                return false;
                            }
                            if let Some(grant) = fb_grant {
                                let eff = if damage.width == 0 || damage.height == 0 {
                                    syscall::DamageRect {
                                        x: 0,
                                        y: 0,
                                        width: buf.width,
                                        height: buf.height,
                                    }
                                } else {
                                    damage
                                };
                                submit_damage_to_fb_server(pid, grant, eff);
                            }
                        }
                    }
                    push_empty_evt(&mut client.tx_buf, attached_buf, 0); // wl_buffer.release
                }
                if need_configure {
                    let serial = client.next_serial();
                    if xdg_top_id != 0 {
                        push_xdg_toplevel_configure(&mut client.tx_buf, xdg_top_id, 0, 0);
                    }
                    push_u32_evt(&mut client.tx_buf, xdg_surf_id, 0, serial);
                }
                if frame_cb != 0 {
                    push_u32_evt(&mut client.tx_buf, frame_cb, 0, 1);
                    push_u32_evt(&mut client.tx_buf, 1, 1, frame_cb);
                }
            }
            // xdg_wm_base.get_xdg_surface(new_id, wl_surface)
            (ObjKind::XdgWmBase, 2) => {
                let xdg_sid = read_u32(&payload, 0);
                let wl_sid = read_u32(&payload, 4);
                if !client.surfaces.contains_key(&wl_sid)
                    || !client.register_obj(xdg_sid, ObjKind::XdgSurface)
                {
                    return false;
                }
                client.xdg_to_surface.insert(xdg_sid, wl_sid);
                if let Some(surf) = client.surfaces.get_mut(&wl_sid) {
                    surf.xdg_surface_id = xdg_sid;
                }
            }
            // xdg_surface.destroy()
            (ObjKind::XdgSurface, 0) => {
                client.xdg_to_surface.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // xdg_surface.get_toplevel(new_id)
            (ObjKind::XdgSurface, 1) => {
                let top_id = read_u32(&payload, 0);
                if !client.register_obj(top_id, ObjKind::XdgToplevel) {
                    return false;
                }
                if let Some(&wl_sid) = client.xdg_to_surface.get(&obj_id) {
                    if let Some(surf) = client.surfaces.get_mut(&wl_sid) {
                        surf.toplevel = true;
                        surf.xdg_toplevel_id = top_id;
                    }
                }
            }
            // xdg_toplevel.destroy()
            (ObjKind::XdgToplevel, 0) => {
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            // wl_seat.get_pointer / get_keyboard
            (ObjKind::Seat, 0) => {
                let ptr_id = read_u32(&payload, 0);
                if !client.register_obj(ptr_id, ObjKind::Pointer) {
                    return false;
                }
            }
            (ObjKind::Seat, 1) => {
                let kbd_id = read_u32(&payload, 0);
                if !client.register_obj(kbd_id, ObjKind::Keyboard) {
                    return false;
                }
            }
            // Other benign no-op opcodes (set_title, set_app_id, ack_configure, pong, set_opaque_region, etc.)
            _ => {}
        }
    }
    if pos > 0 {
        client.rx_buf.drain(..pos);
    }
    true
}

fn bind_wayland_listener() -> Option<i32> {
    let run_dir = b"/run\0";
    let _ = unsafe { syscall::syscall2(syscall::SYS_MKDIR, run_dir.as_ptr() as u64, 0o755) };
    let sock_path = b"/run/wayland-0\0";
    let _ = unsafe { syscall::syscall1(syscall::SYS_UNLINK, sock_path.as_ptr() as u64) };

    let lfd = unsafe {
        syscall::syscall3(
            syscall::SYS_SOCKET,
            AF_UNIX,
            SOCK_STREAM | syscall::O_NONBLOCK | syscall::O_CLOEXEC,
            0,
        )
    };
    if lfd < 0 {
        return None;
    }
    let mut sun = SockAddrUn {
        sun_family: AF_UNIX as u16,
        sun_path: [0u8; 108],
    };
    sun.sun_path[..sock_path.len()].copy_from_slice(sock_path);
    let addr_len = core::mem::size_of::<SockAddrUn>() as u64;
    let brc = unsafe {
        syscall::syscall3(
            syscall::SYS_BIND,
            lfd as u64,
            &sun as *const SockAddrUn as u64,
            addr_len,
        )
    };
    if brc < 0 {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, lfd as u64);
        }
        return None;
    }
    let lrc = unsafe { syscall::syscall2(syscall::SYS_LISTEN, lfd as u64, 16) };
    if lrc < 0 {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, lfd as u64);
        }
        return None;
    }
    Some(lfd as i32)
}

fn main() {
    boot_log(b"exo_compositor: boot (Strate 2 isolated Wayland compositor, 0 MMIO)\n");
    let mut listen_fd = None;
    for _ in 0..20 {
        if let Some(fd) = bind_wayland_listener() {
            listen_fd = Some(fd);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let Some(listen_fd) = listen_fd else {
        boot_log(b"exo_compositor: failed to bind /run/wayland-0\n");
        std::process::exit(1);
    };
    let Some((pid, ep)) = register_ipc_endpoint() else {
        boot_log(b"exo_compositor: failed to register IPC endpoint\n");
        std::process::exit(1);
    };
    boot_log(b"exo_compositor: /run/wayland-0 ready\n");

    let epfd = unsafe { syscall::syscall1(syscall::SYS_EPOLL_CREATE1, 0) } as i32;
    if epfd >= 0 {
        let ev = EpollEvent {
            events: EPOLLIN,
            data: listen_fd as u64,
        };
        let _ = unsafe {
            syscall::syscall4(
                syscall::SYS_EPOLL_CTL,
                epfd as u64,
                EPOLL_CTL_ADD,
                listen_fd as u64,
                &ev as *const EpollEvent as u64,
            )
        };
    }

    let mut fb_grant = request_fb_surface_grant(pid, ep);
    let mut clients: Vec<ClientState> = Vec::new();
    let mut events = [EpollEvent::default(); 16];

    loop {
        if fb_grant.is_none() {
            fb_grant = request_fb_surface_grant(pid, ep);
        }

        if epfd >= 0 {
            let _ = unsafe {
                syscall::syscall4(
                    syscall::SYS_EPOLL_WAIT,
                    epfd as u64,
                    events.as_mut_ptr() as u64,
                    events.len() as u64,
                    16,
                )
            };
        } else {
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        // Accept new Wayland clients up to MAX_WAYLAND_CLIENTS.
        loop {
            let cfd = unsafe { syscall::syscall3(syscall::SYS_ACCEPT, listen_fd as u64, 0, 0) };
            if cfd < 0 {
                break;
            }
            if clients.len() >= MAX_WAYLAND_CLIENTS {
                unsafe {
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, cfd as u64);
                }
                continue;
            }
            if epfd >= 0 {
                let ev = EpollEvent {
                    events: EPOLLIN,
                    data: cfd as u64,
                };
                let _ = unsafe {
                    syscall::syscall4(
                        syscall::SYS_EPOLL_CTL,
                        epfd as u64,
                        EPOLL_CTL_ADD,
                        cfd as u64,
                        &ev as *const EpollEvent as u64,
                    )
                };
            }
            clients.push(ClientState::new(cfd as i32));
        }

        // Poll active clients.
        let mut idx = 0usize;
        while idx < clients.len() {
            let mut disconnected = false;
            {
                let client = &mut clients[idx];
                let mut tmp = [0u8; 2048];
                for _ in 0..4 {
                    if client.rx_buf.len() >= MAX_RX_BUF {
                        disconnected = true;
                        break;
                    }
                    let mut iov = RawIoVec {
                        iov_base: tmp.as_mut_ptr() as u64,
                        iov_len: tmp.len() as u64,
                    };
                    let mut ctrl = [0u8; 128];
                    let mut msg = UnixMsghdr {
                        msg_name: 0,
                        msg_namelen: 0,
                        _pad0: 0,
                        msg_iov: &mut iov as *mut RawIoVec as u64,
                        msg_iovlen: 1,
                        msg_control: ctrl.as_mut_ptr() as u64,
                        msg_controllen: ctrl.len() as u64,
                        msg_flags: 0,
                        _pad1: 0,
                    };
                    let rc = unsafe {
                        syscall::syscall3(
                            syscall::SYS_RECVMSG,
                            client.fd as u64,
                            &mut msg as *mut UnixMsghdr as u64,
                            MSG_DONTWAIT,
                        )
                    };
                    if rc < 0 {
                        if rc != syscall::EAGAIN && rc != EINTR_ERR {
                            disconnected = true;
                        }
                        break;
                    }
                    if msg.msg_controllen >= core::mem::size_of::<UnixCmsghdr>() as u64 {
                        let cmsg =
                            unsafe { core::ptr::read_unaligned(ctrl.as_ptr() as *const UnixCmsghdr) };
                        if cmsg.cmsg_level == 1 && cmsg.cmsg_type == 1 && cmsg.cmsg_len >= 16 {
                            let avail = (msg.msg_controllen as usize)
                                .min(ctrl.len())
                                .min(cmsg.cmsg_len);
                            let n_fds = avail.saturating_sub(16) / 4;
                            for fi in 0..n_fds {
                                let off = 16 + fi * 4;
                                let rfd = i32::from_le_bytes([
                                    ctrl[off],
                                    ctrl[off + 1],
                                    ctrl[off + 2],
                                    ctrl[off + 3],
                                ]);
                                if client.rx_fds.len() < 16 {
                                    client.rx_fds.push_back(rfd);
                                } else if rfd >= 0 {
                                    unsafe {
                                        let _ = syscall::syscall1(syscall::SYS_CLOSE, rfd as u64);
                                    }
                                }
                            }
                        }
                    }
                    if rc == 0 && msg.msg_controllen == 0 {
                        disconnected = true;
                        break;
                    }
                    client.rx_buf.extend_from_slice(&tmp[..rc as usize]);
                }

                if !disconnected && client.rx_buf.len() >= 8 {
                    if !dispatch_client_messages(client, pid, fb_grant.as_ref()) {
                        disconnected = true;
                    }
                }

                if !disconnected && !client.tx_buf.is_empty() {
                    let iov = RawIoVec {
                        iov_base: client.tx_buf.as_ptr() as u64,
                        iov_len: client.tx_buf.len() as u64,
                    };
                    let msg = UnixMsghdr {
                        msg_name: 0,
                        msg_namelen: 0,
                        _pad0: 0,
                        msg_iov: &iov as *const RawIoVec as u64,
                        msg_iovlen: 1,
                        msg_control: 0,
                        msg_controllen: 0,
                        msg_flags: 0,
                        _pad1: 0,
                    };
                    let sent = unsafe {
                        syscall::syscall3(
                            syscall::SYS_SENDMSG,
                            client.fd as u64,
                            &msg as *const UnixMsghdr as u64,
                            MSG_DONTWAIT,
                        )
                    };
                    if sent > 0 {
                        let s = (sent as usize).min(client.tx_buf.len());
                        client.tx_buf.drain(..s);
                    } else if sent < 0 && sent != syscall::EAGAIN && sent != EINTR_ERR {
                        disconnected = true;
                    }
                }
            }

            if disconnected {
                let cfd = clients[idx].fd;
                if epfd >= 0 && cfd >= 0 {
                    let _ = unsafe {
                        syscall::syscall4(
                            syscall::SYS_EPOLL_CTL,
                            epfd as u64,
                            EPOLL_CTL_DEL,
                            cfd as u64,
                            0,
                        )
                    };
                }
                let mut removed = clients.swap_remove(idx);
                removed.cleanup();
            } else {
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_bounds_rejects_stride_and_offset_overflows() {
        assert!(validate_shm_buffer_bounds(0, 64, 64, 256, 4, 64 * 64 * 4));
        assert!(!validate_shm_buffer_bounds(0, 64, 64, 128, 4, 64 * 64 * 4)); // stride < width * bpp
        assert!(!validate_shm_buffer_bounds(0, 64, 64, u32::MAX, 4, 16 * 1024 * 1024));
        assert!(!validate_shm_buffer_bounds(u32::MAX, 64, 64, 256, 4, 16 * 1024 * 1024));
        assert!(!validate_shm_buffer_bounds(4096, 64, 64, 256, 4, 64 * 64 * 4)); // end > pool
    }

    #[test]
    fn client_object_quota_enforced_without_panic() {
        let mut client = ClientState::new(-1);
        for id in 2..(MAX_OBJECTS_PER_CLIENT as u32 + 1) {
            assert!(client.register_obj(id, ObjKind::Region));
        }
        assert!(!client.register_obj(999, ObjKind::Region));
    }
}
