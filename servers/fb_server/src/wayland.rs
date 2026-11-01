#![allow(dead_code)]

//! # `wayland.rs` — Moteur du protocole filaire Wayland (`#![no_std]`) pour `fb_server`
//!
//! Implémente le protocole binaire Wayland (en-tête 8 octets `[object_id: u32, (size << 16) | opcode: u32]`)
//! sur IPC Exo-OS + partage mémoire zéro-copie (`SurfaceGrant` / `DamageRect`) :
//! - `wl_display` (id = 1) : `sync` (0), `get_registry` (1)
//! - `wl_registry` : `bind` (0) -> annonce `wl_compositor`, `wl_shm`, `xdg_wm_base`, `wl_seat`
//! - `wl_compositor` : `create_surface` (0)
//! - `wl_shm` / `wl_shm_pool` / `wl_buffer` : tampon ARGB8888 / XRGB8888 partagé via `memory_server`
//! - `wl_surface` : `attach` (1), `damage` (2), `frame` (3), `commit` (6), `damage_buffer` (9)
//! - `xdg_wm_base` / `xdg_surface` / `xdg_toplevel` : gestion de fenêtres de bureau
//! - `wl_seat` / `wl_pointer` / `wl_keyboard` : routage d'événements d'entrée

use exo_syscall_abi as syscall;
use crate::Framebuffer;

pub const MAX_WAYLAND_OBJECTS: usize = 32;
pub const MAX_SHM_SURFACES: usize = 8;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WlObjectKind {
    Free = 0,
    Display = 1,
    Registry = 2,
    Compositor = 3,
    Shm = 4,
    ShmPool = 5,
    Buffer = 6,
    Surface = 7,
    XdgWmBase = 8,
    XdgSurface = 9,
    XdgToplevel = 10,
    Seat = 11,
    Pointer = 12,
    Keyboard = 13,
    Callback = 14,
}

#[derive(Clone, Copy)]
pub struct WlObject {
    pub id: u32,
    pub kind: WlObjectKind,
    pub owner_pid: u32,
    pub surface_idx: u8,
}

impl WlObject {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            kind: WlObjectKind::Free,
            owner_pid: 0,
            surface_idx: 0xFF,
        }
    }
}

#[derive(Clone, Copy)]
pub struct CompositorSurface {
    pub active: bool,
    pub surface_id: u32,
    pub owner_pid: u32,
    pub shm_handle: u64,
    pub virt_addr: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub format: u32,
    pub z_order: u8,
    pub damaged: bool,
    pub damage: syscall::DamageRect,
    pub title: [u8; 32],
    pub title_len: u8,
}

impl CompositorSurface {
    pub const fn empty() -> Self {
        Self {
            active: false,
            surface_id: 0,
            owner_pid: 0,
            shm_handle: 0,
            virt_addr: 0,
            x: 60,
            y: 56,
            width: 0,
            height: 0,
            stride_bytes: 0,
            format: syscall::PixelFormat::Bgrx8888 as u32,
            z_order: 1,
            damaged: false,
            damage: syscall::DamageRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            title: [0; 32],
            title_len: 0,
        }
    }
}

pub struct WaylandState {
    pub objects: [WlObject; MAX_WAYLAND_OBJECTS],
    pub surfaces: [CompositorSurface; MAX_SHM_SURFACES],
    pub next_surface_id: u32,
    pub wire_messages_processed: u64,
}

impl WaylandState {
    pub const fn new() -> Self {
        let mut objects = [WlObject::empty(); MAX_WAYLAND_OBJECTS];
        objects[0] = WlObject {
            id: 1,
            kind: WlObjectKind::Display,
            owner_pid: 0,
            surface_idx: 0xFF,
        };
        Self {
            objects,
            surfaces: [CompositorSurface::empty(); MAX_SHM_SURFACES],
            next_surface_id: 1,
            wire_messages_processed: 0,
        }
    }

    fn find_object(&self, id: u32) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_WAYLAND_OBJECTS {
            if self.objects[i].kind != WlObjectKind::Free && self.objects[i].id == id {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn register_object(&mut self, id: u32, kind: WlObjectKind, owner_pid: u32, surface_idx: u8) -> bool {
        if let Some(idx) = self.find_object(id) {
            self.objects[idx] = WlObject {
                id,
                kind,
                owner_pid,
                surface_idx,
            };
            return true;
        }
        let mut i = 0usize;
        while i < MAX_WAYLAND_OBJECTS {
            if self.objects[i].kind == WlObjectKind::Free {
                self.objects[i] = WlObject {
                    id,
                    kind,
                    owner_pid,
                    surface_idx,
                };
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn alloc_surface(&mut self, owner_pid: u32, width: u32, height: u32) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if !self.surfaces[i].active {
                let id = self.next_surface_id;
                self.next_surface_id = self.next_surface_id.wrapping_add(1).max(1);
                let stride_bytes = width.saturating_mul(4);
                let size_bytes = (stride_bytes as u64).saturating_mul(height as u64).max(4096);
                let virt_rc = unsafe {
                    syscall::syscall6(
                        syscall::SYS_MMAP,
                        0,
                        size_bytes,
                        3,    // PROT_READ | PROT_WRITE
                        0x22, // MAP_PRIVATE | MAP_ANONYMOUS
                        u64::MAX,
                        0,
                    )
                };
                let virt_addr = if virt_rc > 0 { virt_rc as u64 } else { 0 };
                let mut client_handle = 0u64;
                if owner_pid > 1 {
                    let client_virt = unsafe {
                        syscall::syscall5(
                            syscall::SYS_EXO_MEM_MAP_PID,
                            owner_pid as u64,
                            0,
                            size_bytes,
                            3,    // PROT_READ | PROT_WRITE
                            0x22, // MAP_PRIVATE | MAP_ANONYMOUS
                        )
                    };
                    if client_virt > 0 {
                        client_handle = client_virt as u64;
                    }
                }
                let mut s = CompositorSurface::empty();
                s.active = true;
                s.surface_id = id;
                s.owner_pid = owner_pid;
                s.shm_handle = client_handle;
                s.virt_addr = virt_addr;
                s.width = width;
                s.height = height;
                s.stride_bytes = stride_bytes;
                s.damaged = true;
                s.damage = syscall::DamageRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                };
                s.title[..11].copy_from_slice(b"ExoSurface\0");
                s.title_len = 10;
                self.surfaces[i] = s;
                return Some(i);
            }
            i += 1;
        }
        None
    }

    pub fn destroy_surface(&mut self, surface_id: u32, sender_pid: u32) -> bool {
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].surface_id == surface_id {
                if sender_pid != 0 && sender_pid != 1 && self.surfaces[i].owner_pid != sender_pid {
                    return false;
                }
                self.surfaces[i] = CompositorSurface::empty();
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn commit_surface_damage(
        &mut self,
        surface_id: u32,
        sender_pid: u32,
        rect: syscall::DamageRect,
        fb: Framebuffer,
    ) -> Option<(u32, u32)> {
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].surface_id == surface_id {
                if sender_pid != 0 && sender_pid != 1 && self.surfaces[i].owner_pid != sender_pid {
                    return None;
                }
                self.surfaces[i].damaged = true;
                self.surfaces[i].damage = rect;

                // Si la surface possède un buffer distant dans l'espace client (shm_handle),
                // copier la zone modifiée vers le buffer local de fb_server
                let s = &self.surfaces[i];
                if s.shm_handle != 0 && s.virt_addr != 0 && s.owner_pid > 1 {
                    let dmg_y = rect.y.min(s.height);
                    let dmg_h = if rect.height == 0 { s.height } else { rect.height }.min(s.height.saturating_sub(dmg_y));
                    let stride = s.stride_bytes as u64;
                    let mut r = 0u32;
                    while r < dmg_h {
                        let row_offset = ((dmg_y + r) as u64).saturating_mul(stride);
                        let remote_src = s.shm_handle.saturating_add(row_offset);
                        let local_dst = s.virt_addr.saturating_add(row_offset);
                        unsafe {
                            let _ = syscall::syscall6(
                                syscall::SYS_EXO_MEM_COPY_FROM_PID,
                                s.owner_pid as u64,
                                remote_src,
                                local_dst,
                                stride,
                                0,
                                0,
                            );
                        }
                        r += 1;
                    }
                }

                self.composite_surface(i, fb);

                let y_start = (self.surfaces[i].y + rect.y as i32).max(0) as u32;
                let rect_h = if rect.height == 0 { self.surfaces[i].height } else { rect.height };
                let y_end = (self.surfaces[i].y + (rect.y + rect_h) as i32).max(0) as u32;
                return Some((y_start, y_end));
            }
            i += 1;
        }
        None
    }

    pub fn composite_surface(&self, s_idx: usize, fb: Framebuffer) {
        if s_idx >= MAX_SHM_SURFACES || !self.surfaces[s_idx].active || self.surfaces[s_idx].virt_addr == 0 {
            return;
        }
        let s = &self.surfaces[s_idx];
        let base = fb.draw_target_base();
        if base == 0 || s.width == 0 || s.height == 0 {
            return;
        }
        let bpp = fb.bytes_per_pixel() as usize;
        let dst_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let src_stride = s.stride_bytes as usize;

        let dmg_x = s.damage.x.min(s.width);
        let dmg_y = s.damage.y.min(s.height);
        let dmg_w = if s.damage.width == 0 { s.width } else { s.damage.width }.min(s.width.saturating_sub(dmg_x));
        let dmg_h = if s.damage.height == 0 { s.height } else { s.damage.height }.min(s.height.saturating_sub(dmg_y));

        let mut row = 0u32;
        while row < dmg_h {
            let sy = dmg_y + row;
            let dy = s.y + sy as i32;
            if dy >= 0 && (dy as u32) < fb.height {
                let dx = s.x + dmg_x as i32;
                if dx >= 0 && (dx as u32) < fb.width {
                    let clip_w = dmg_w.min(fb.width.saturating_sub(dx as u32));
                    let src_offset = (sy as usize).saturating_mul(src_stride) + (dmg_x as usize) * 4;
                    let dst_offset = (dy as usize).saturating_mul(dst_stride) + (dx as usize) * bpp;
                    let src_ptr = (s.virt_addr as usize + src_offset) as *const u8;
                    let dst_ptr = (base + dst_offset) as *mut u8;
                    unsafe {
                        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, (clip_w as usize) * bpp);
                    }
                }
            }
            row += 1;
        }
    }

    /// Décode et traite un flux de messages filaires Wayland (`[object_id: u32, size_opcode: u32, payload...]`).
    pub fn dispatch_wire(&mut self, sender_pid: u32, bytes: &[u8], fb: Framebuffer) -> (i64, Option<(u32, u32)>) {
        let mut offset = 0usize;
        let mut processed = 0i64;
        let mut dirty_min = u32::MAX;
        let mut dirty_max = 0u32;
        while offset + 8 <= bytes.len() {
            let object_id = read_u32(bytes, offset);
            let size_op = read_u32(bytes, offset + 4);
            let msg_size = ((size_op >> 16) as usize).max(8);
            let opcode = (size_op & 0xFFFF) as u16;
            if offset + msg_size > bytes.len() {
                break;
            }
            let payload = &bytes[offset + 8..offset + msg_size];
            if let Some((y0, y1)) = self.handle_single_wire_msg(sender_pid, object_id, opcode, payload, fb) {
                if y0 < dirty_min {
                    dirty_min = y0;
                }
                if y1 > dirty_max {
                    dirty_max = y1;
                }
            }
            offset += msg_size;
            processed += 1;
            self.wire_messages_processed = self.wire_messages_processed.saturating_add(1);
        }
        let dirty_bounds = if dirty_min < dirty_max {
            Some((dirty_min, dirty_max))
        } else {
            None
        };
        (processed, dirty_bounds)
    }

    fn handle_single_wire_msg(
        &mut self,
        sender_pid: u32,
        object_id: u32,
        opcode: u16,
        payload: &[u8],
        fb: Framebuffer,
    ) -> Option<(u32, u32)> {
        let Some(obj_idx) = self.find_object(object_id) else {
            return None;
        };
        let kind = self.objects[obj_idx].kind;
        let surf_idx = self.objects[obj_idx].surface_idx;

        match (kind, opcode) {
            // wl_display.sync(new_id callback)
            (WlObjectKind::Display, 0) => {
                let cb_id = read_u32(payload, 0);
                if cb_id != 0 {
                    let _ = self.register_object(cb_id, WlObjectKind::Callback, sender_pid, 0xFF);
                }
            }
            // wl_display.get_registry(new_id registry)
            (WlObjectKind::Display, 1) => {
                let reg_id = read_u32(payload, 0);
                if reg_id != 0 {
                    let _ = self.register_object(reg_id, WlObjectKind::Registry, sender_pid, 0xFF);
                }
            }
            // wl_registry.bind(name, new_id)
            (WlObjectKind::Registry, 0) => {
                let name = read_u32(payload, 0);
                let new_id = if payload.len() >= 8 {
                    read_u32(payload, payload.len() - 4)
                } else {
                    0
                };
                if new_id != 0 {
                    let target_kind = match name {
                        1 => WlObjectKind::Compositor,
                        2 => WlObjectKind::Shm,
                        3 => WlObjectKind::XdgWmBase,
                        4 => WlObjectKind::Seat,
                        _ => WlObjectKind::Compositor,
                    };
                    let _ = self.register_object(new_id, target_kind, sender_pid, 0xFF);
                }
            }
            // wl_compositor.create_surface(new_id surface)
            (WlObjectKind::Compositor, 0) => {
                let surf_id = read_u32(payload, 0);
                if surf_id != 0 {
                    if let Some(s_idx) = self.alloc_surface(sender_pid, 320, 200) {
                        let _ = self.register_object(surf_id, WlObjectKind::Surface, sender_pid, s_idx as u8);
                    }
                }
            }
            // wl_shm.create_pool(new_id pool, fd, size)
            (WlObjectKind::Shm, 0) => {
                let pool_id = read_u32(payload, 0);
                if pool_id != 0 {
                    let _ = self.register_object(pool_id, WlObjectKind::ShmPool, sender_pid, 0xFF);
                }
            }
            // wl_shm_pool.create_buffer(new_id buffer, offset, width, height, stride, format)
            (WlObjectKind::ShmPool, 0) => {
                let buf_id = read_u32(payload, 0);
                if buf_id != 0 {
                    let _ = self.register_object(buf_id, WlObjectKind::Buffer, sender_pid, 0xFF);
                }
            }
            // wl_surface.attach(buffer_id, x, y)
            (WlObjectKind::Surface, 1) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    self.surfaces[surf_idx as usize].damaged = true;
                }
            }
            // wl_surface.damage / damage_buffer(x, y, w, h)
            (WlObjectKind::Surface, 2 | 9) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES && payload.len() >= 16 {
                    let s = &mut self.surfaces[surf_idx as usize];
                    s.damage = syscall::DamageRect {
                        x: read_u32(payload, 0),
                        y: read_u32(payload, 4),
                        width: read_u32(payload, 8),
                        height: read_u32(payload, 12),
                    };
                }
            }
            // wl_surface.commit() -> composite et synchronise les lignes vers le framebuffer
            (WlObjectKind::Surface, 6) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let s_id = self.surfaces[surf_idx as usize].surface_id;
                    let rect = self.surfaces[surf_idx as usize].damage;
                    return self.commit_surface_damage(s_id, sender_pid, rect, fb);
                }
            }
            // xdg_wm_base.get_xdg_surface(new_id, wl_surface)
            (WlObjectKind::XdgWmBase, 2) => {
                let xdg_surf_id = read_u32(payload, 0);
                let wl_surf_id = read_u32(payload, 4);
                let s_idx = self
                    .find_object(wl_surf_id)
                    .map(|idx| self.objects[idx].surface_idx)
                    .unwrap_or(0xFF);
                if xdg_surf_id != 0 {
                    let _ = self.register_object(xdg_surf_id, WlObjectKind::XdgSurface, sender_pid, s_idx);
                }
            }
            // xdg_surface.get_toplevel(new_id)
            (WlObjectKind::XdgSurface, 1) => {
                let top_id = read_u32(payload, 0);
                if top_id != 0 {
                    let _ = self.register_object(top_id, WlObjectKind::XdgToplevel, sender_pid, surf_idx);
                }
            }
            // wl_seat.get_pointer(new_id) / get_keyboard(new_id)
            (WlObjectKind::Seat, 0) => {
                let ptr_id = read_u32(payload, 0);
                if ptr_id != 0 {
                    let _ = self.register_object(ptr_id, WlObjectKind::Pointer, sender_pid, 0xFF);
                }
            }
            (WlObjectKind::Seat, 1) => {
                let kbd_id = read_u32(payload, 0);
                if kbd_id != 0 {
                    let _ = self.register_object(kbd_id, WlObjectKind::Keyboard, sender_pid, 0xFF);
                }
            }
            _ => {}
        }
        None
    }
}

#[inline]
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    if offset + 4 > bytes.len() {
        return 0;
    }
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
