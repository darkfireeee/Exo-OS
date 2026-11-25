//! Wayland wire protocol state machine and `/run/wayland-0` socket listener.

use crate::fb_grant_client::{submit_damage_to_fb_server, FbSurfaceGrant};
use crate::shm_pool::{
    format_bpp, map_verified_shm_fd, unmap_close_pool, validate_shm_buffer_bounds, ShmPool,
    WlBuffer, MAX_POOLS_PER_CLIENT, MAX_SHM_BYTES_PER_CLIENT,
};
use exo_syscall_abi as syscall;
use std::collections::{HashMap, VecDeque};

pub const MAX_OBJECTS_PER_CLIENT: usize = 32;
pub const MAX_WIRE_MSG_SIZE: usize = 4096;

const AF_UNIX: u64 = 1;
const SOCK_STREAM: u64 = 1;

#[repr(C)]
pub struct SockAddrUn {
    pub sun_family: u16,
    pub sun_path: [u8; 108],
}

#[repr(C)]
pub struct RawIoVec {
    pub iov_base: u64,
    pub iov_len: u64,
}

#[repr(C)]
pub struct UnixMsghdr {
    pub msg_name: u64,
    pub msg_namelen: u32,
    pub _pad0: u32,
    pub msg_iov: u64,
    pub msg_iovlen: u64,
    pub msg_control: u64,
    pub msg_controllen: u64,
    pub msg_flags: i32,
    pub _pad1: u32,
}

#[repr(C)]
pub struct UnixCmsghdr {
    pub cmsg_len: usize,
    pub cmsg_level: i32,
    pub cmsg_type: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjKind {
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

#[derive(Clone, Debug, Default)]
pub struct WlSurface {
    pub attached_buffer: u32,
    pub xdg_surface_id: u32,
    pub xdg_toplevel_id: u32,
    pub frame_callback_id: u32,
    pub configure_sent: bool,
    pub toplevel: bool,
    pub damage: syscall::DamageRect,
}

pub struct ClientState {
    pub fd: i32,
    pub rx_buf: Vec<u8>,
    pub tx_buf: Vec<u8>,
    pub rx_fds: VecDeque<i32>,
    pub objects: HashMap<u32, ObjKind>,
    pub pools: HashMap<u32, ShmPool>,
    pub buffers: HashMap<u32, WlBuffer>,
    pub surfaces: HashMap<u32, WlSurface>,
    pub xdg_to_surface: HashMap<u32, u32>,
    pub next_serial: u32,
}

impl ClientState {
    pub fn new(fd: i32) -> Self {
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

    pub fn total_mapped_shm(&self) -> u64 {
        self.pools
            .values()
            .fold(0u64, |acc, p| acc.saturating_add(p.mapped_len))
    }

    pub fn register_obj(&mut self, id: u32, kind: ObjKind) -> bool {
        if id == 0 {
            return false;
        }
        if !self.objects.contains_key(&id) && self.objects.len() >= MAX_OBJECTS_PER_CLIENT {
            return false;
        }
        self.objects.insert(id, kind);
        true
    }

    pub fn next_serial(&mut self) -> u32 {
        let s = self.next_serial.max(1);
        self.next_serial = s.wrapping_add(1).max(1);
        s
    }

    pub fn cleanup(&mut self) {
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
    tx.extend_from_slice(&0u32.to_le_bytes());
}

pub fn dispatch_client_messages(
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
            (ObjKind::Display, 0) => {
                let cb_id = read_u32(&payload, 0);
                if cb_id == 0 {
                    return false;
                }
                push_u32_evt(&mut client.tx_buf, cb_id, 0, 0);
                push_u32_evt(&mut client.tx_buf, 1, 1, cb_id);
            }
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
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 0);
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 1);
                    }
                    ObjKind::Seat => {
                        push_u32_evt(&mut client.tx_buf, new_id, 0, 3);
                        push_str_evt(&mut client.tx_buf, new_id, 1, b"seat0");
                    }
                    _ => {}
                }
            }
            (ObjKind::Compositor, 0) => {
                let sid = read_u32(&payload, 0);
                if !client.register_obj(sid, ObjKind::Surface) {
                    return false;
                }
                client.surfaces.insert(sid, WlSurface::default());
            }
            (ObjKind::Compositor, 1) => {
                let rid = read_u32(&payload, 0);
                if !client.register_obj(rid, ObjKind::Region) {
                    return false;
                }
            }
            (ObjKind::Compositor, 2)
            | (ObjKind::Region, 0)
            | (ObjKind::Shm, 1)
            | (ObjKind::XdgWmBase, 0)
            | (ObjKind::Seat, 3) => {
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
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
            (ObjKind::ShmPool, 1) => {
                if let Some(pool) = client.pools.remove(&obj_id) {
                    unmap_close_pool(pool);
                }
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
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
            (ObjKind::Buffer, 0) => {
                client.buffers.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            (ObjKind::Surface, 0) => {
                client.surfaces.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
            (ObjKind::Surface, 1) => {
                let buf_id = read_u32(&payload, 0);
                if buf_id != 0 && !client.buffers.contains_key(&buf_id) {
                    return false;
                }
                if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                    surf.attached_buffer = buf_id;
                }
            }
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
            (ObjKind::Surface, 3) => {
                let cb_id = read_u32(&payload, 0);
                if let Some(surf) = client.surfaces.get_mut(&obj_id) {
                    surf.frame_callback_id = cb_id;
                }
            }
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
                    push_empty_evt(&mut client.tx_buf, attached_buf, 0);
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
            (ObjKind::XdgSurface, 0) => {
                client.xdg_to_surface.remove(&obj_id);
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
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
            (ObjKind::XdgToplevel, 0) => {
                client.objects.remove(&obj_id);
                push_u32_evt(&mut client.tx_buf, 1, 1, obj_id);
            }
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
            _ => {}
        }
    }
    if pos > 0 {
        client.rx_buf.drain(..pos);
    }
    true
}

pub fn bind_wayland_listener() -> Option<i32> {
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
