#![allow(dead_code)]

//! # `wayland.rs` — Moteur du protocole filaire Wayland (`#![no_std]`) pour `fb_server`
//!
//! Implémente le protocole binaire Wayland (en-tête 8 octets `[object_id: u32, (size << 16) | opcode: u32]`)
//! sur IPC Exo-OS + partage mémoire zéro-copie (`SurfaceGrant` / `DamageRect`) :
//! - `wl_display` (id = 1) : `sync` (0), `get_registry` (1)
//! - `wl_registry` : `bind` (0) -> annonce `wl_compositor`, `wl_shm`, `xdg_wm_base`, `wl_seat`
//! - `wl_compositor` : `create_surface` (0)
//! - `wl_shm` / `wl_shm_pool` / `wl_buffer` : gestion multi-buffers (`offset`, `width`, `height`,
//!   `stride`, `format`) avec décodage complet `ARGB8888`, `XRGB8888`, `ABGR8888`, `XBGR8888`,
//!   `RGB565`, `RGB888`, `BGR888` en 24bpp et 32bpp.
//! - `wl_surface` : `attach` (1), `damage` (2), `frame` (3), `commit` (6), `damage_buffer` (9)
//! - `xdg_wm_base` / `xdg_surface` / `xdg_toplevel` : déplacement, redimensionnement,
//!   minimisation, maximisation, fermeture et focus.
//! - `wl_seat` / `wl_pointer` / `wl_keyboard` : suivi pointeur par surface et drag/clic bureau.

use crate::desktop;
use crate::Framebuffer;
use exo_syscall_abi as syscall;

pub const MAX_WAYLAND_OBJECTS: usize = 192;
pub const MAX_SHM_SURFACES: usize = 8;
pub const MAX_SHM_POOLS: usize = 16;
pub const MAX_WL_BUFFERS: usize = 32;
pub const CLIENT_OWNER_BASE: u32 = 0x8000_0000;
pub const MAX_WAYLAND_CLIENTS: usize = 8;
pub const WL_CLIENT_RX_CAP: usize = 2048;
pub const WL_CLIENT_TX_CAP: usize = 2048;
pub const WL_CLIENT_FD_CAP: usize = 16;

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
    Region = 15,
}

#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShmPixelEncoding {
    Argb8888 = 0,
    Xrgb8888 = 1,
    Abgr8888 = 2,
    Xbgr8888 = 3,
    Rgb565 = 4,
    Rgb888 = 5,
    Bgr888 = 6,
}

impl ShmPixelEncoding {
    pub fn from_wl_shm_format(fmt: u32) -> Self {
        match fmt {
            0 | 0x34325241 => Self::Argb8888, // ARGB8888 / 'AR24'
            1 | 0x34325258 => Self::Xrgb8888, // XRGB8888 / 'XR24'
            2 | 0x34324241 => Self::Abgr8888, // ABGR8888 / 'AB24'
            3 | 0x34324258 => Self::Xbgr8888, // XBGR8888 / 'XB24'
            4 | 0x36314752 => Self::Rgb565,   // RGB565 / 'RG16'
            5 | 0x34324752 => Self::Rgb888,   // RGB888 / 'RG24'
            6 | 0x34324742 => Self::Bgr888,   // BGR888 / 'BG24'
            _ => Self::Argb8888,
        }
    }

    #[inline]
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgb565 => 2,
            Self::Rgb888 | Self::Bgr888 => 3,
            _ => 4,
        }
    }
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
pub struct ShmPoolInfo {
    pub active: bool,
    pub obj_id: u32,
    pub owner_pid: u32,
    pub fd: i32,
    pub size: u32,
    pub local_addr: u64,
    pub mapped_len: u64,
}

impl ShmPoolInfo {
    pub const fn empty() -> Self {
        Self {
            active: false,
            obj_id: 0,
            owner_pid: 0,
            fd: -1,
            size: 0,
            local_addr: 0,
            mapped_len: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct WlBufferInfo {
    pub active: bool,
    pub obj_id: u32,
    pub owner_pid: u32,
    pub pool_id: u32,
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub encoding: ShmPixelEncoding,
}

impl WlBufferInfo {
    pub const fn empty() -> Self {
        Self {
            active: false,
            obj_id: 0,
            owner_pid: 0,
            pool_id: 0,
            offset: 0,
            width: 0,
            height: 0,
            stride: 0,
            encoding: ShmPixelEncoding::Argb8888,
        }
    }
}

#[derive(Clone, Copy)]
pub struct PointerEventRecord {
    pub valid: bool,
    pub surface_idx: u8,
    pub surface_id: u32,
    pub sx: i32,
    pub sy: i32,
    pub button: u32,
    pub pressed: bool,
}

impl PointerEventRecord {
    pub const fn empty() -> Self {
        Self {
            valid: false,
            surface_idx: 0xFF,
            surface_id: 0,
            sx: 0,
            sy: 0,
            button: 0,
            pressed: false,
        }
    }
}

#[derive(Clone, Copy)]
pub enum UiAction {
    None,
    Redraw(u32, u32),
    StartDrag {
        surf_idx: usize,
        off_x: i32,
        off_y: i32,
    },
    StartResize {
        surf_idx: usize,
        start_mx: u32,
        start_my: u32,
        orig_w: u32,
        orig_h: u32,
    },
    StartTermSelect {
        surf_idx: usize,
    },
    Kickoff(desktop::KickoffAction),
    ClientPointer {
        surf_idx: usize,
        sx: i32,
        sy: i32,
    },
}

#[derive(Clone, Copy)]
pub struct CompositorSurface {
    pub active: bool,
    pub surface_id: u32,
    pub owner_pid: u32,
    pub shm_handle: u64,
    pub virt_addr: u64,
    pub buf_size_bytes: u64,
    pub buffer_offset: u32,
    pub attached_buffer: u32,
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
    pub app_id: [u8; 32],
    pub app_id_len: u8,
    pub toplevel: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub focused: bool,
    pub is_terminal: bool,
    pub committed: bool,
    pub xdg_surface_id: u32,
    pub xdg_toplevel_id: u32,
    pub frame_callback_id: u32,
    pub configure_sent: bool,
}

impl CompositorSurface {
    pub const fn empty() -> Self {
        Self {
            active: false,
            surface_id: 0,
            owner_pid: 0,
            shm_handle: 0,
            virt_addr: 0,
            buf_size_bytes: 0,
            buffer_offset: 0,
            attached_buffer: 0,
            x: 80,
            y: 72,
            width: 0,
            height: 0,
            stride_bytes: 0,
            format: ShmPixelEncoding::Argb8888 as u32,
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
            app_id: [0; 32],
            app_id_len: 0,
            toplevel: false,
            minimized: false,
            maximized: false,
            focused: false,
            is_terminal: false,
            committed: false,
            xdg_surface_id: 0,
            xdg_toplevel_id: 0,
            frame_callback_id: 0,
            configure_sent: false,
        }
    }

    pub fn title_str(&self) -> &[u8] {
        let n = (self.title_len as usize).min(self.title.len());
        if n == 0 {
            b"ExoSurface"
        } else {
            &self.title[..n]
        }
    }

    pub fn app_id_str(&self) -> &[u8] {
        let n = (self.app_id_len as usize).min(self.app_id.len());
        &self.app_id[..n]
    }
}

pub struct WaylandState {
    pub objects: [WlObject; MAX_WAYLAND_OBJECTS],
    pub surfaces: [CompositorSurface; MAX_SHM_SURFACES],
    pub pools: [ShmPoolInfo; MAX_SHM_POOLS],
    pub buffers: [WlBufferInfo; MAX_WL_BUFFERS],
    pub last_pointer: PointerEventRecord,
    pub next_surface_id: u32,
    pub next_z: u8,
    pub wire_messages_processed: u64,
    pub kickoff_open: bool,
    pub selected_category: u8,
    pub selected_app: u8,
    pub focused_surface_idx: u8,
    pub next_configure_serial: u32,
}

const fn init_bytes_32<const N: usize>(src: &[u8; N]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut i = 0usize;
    while i < N && i < 32 {
        out[i] = src[i];
        i += 1;
    }
    out
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

        let mut surfaces = [CompositorSurface::empty(); MAX_SHM_SURFACES];
        let mut term = CompositorSurface::empty();
        term.active = true;
        term.surface_id = 1;
        term.owner_pid = 0;
        term.x = desktop::WIN_LEFT_X as i32;
        term.y = desktop::WIN_TOP_Y as i32;
        term.width = 0;
        term.height = 0;
        term.z_order = 2;
        term.toplevel = true;
        term.focused = true;
        term.is_terminal = true;
        term.committed = true;
        term.title = init_bytes_32(b"ExoTerm - exosh (Wayland)");
        term.title_len = 25;
        term.app_id = init_bytes_32(b"ExoTerm");
        term.app_id_len = 7;
        surfaces[0] = term;

        Self {
            objects,
            surfaces,
            pools: [ShmPoolInfo::empty(); MAX_SHM_POOLS],
            buffers: [WlBufferInfo::empty(); MAX_WL_BUFFERS],
            last_pointer: PointerEventRecord::empty(),
            next_surface_id: 2,
            next_z: 3,
            wire_messages_processed: 0,
            kickoff_open: true,
            selected_category: 0, // Favorites
            selected_app: 0,
            focused_surface_idx: 0,
            next_configure_serial: 1,
        }
    }

    pub fn ensure_terminal_bounds(&mut self, fb: Framebuffer) {
        if !fb.is_present() {
            return;
        }
        let s = &mut self.surfaces[0];
        if s.width == 0 || s.height == 0 {
            let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
            if fb.width >= 1280 && fb.height >= 720 {
                // Full-HD layout: Kickoff bottom-left (x=8..468), Terminal clean on center-right
                s.x = desktop::WIN_LEFT_X as i32;
                s.y = desktop::WIN_TOP_Y as i32;
                s.width = fb.width.saturating_sub(desktop::WIN_LEFT_X + 36).min(1360);
                s.height = desktop_h.saturating_sub(desktop::WIN_TOP_Y + 32).min(920);
            } else {
                s.x = 374.min(fb.width.saturating_sub(340) as i32).max(4);
                s.y = 140.min(desktop_h.saturating_sub(240) as i32).max(4);
                s.width = fb.width.saturating_sub(s.x as u32 + 14).max(320);
                s.height = desktop_h.saturating_sub(s.y as u32 + 12).max(200);
            }
        }
    }

    fn find_object(&self, owner_pid: u32, id: u32) -> Option<usize> {
        if id == 1 {
            return Some(0);
        }
        let strict_owner = owner_pid >= CLIENT_OWNER_BASE;
        let mut fallback: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_WAYLAND_OBJECTS {
            if self.objects[i].kind != WlObjectKind::Free && self.objects[i].id == id {
                if self.objects[i].owner_pid == owner_pid {
                    return Some(i);
                }
                if !strict_owner && self.objects[i].owner_pid < CLIENT_OWNER_BASE && fallback.is_none() {
                    fallback = Some(i);
                }
            }
            i += 1;
        }
        fallback
    }

    fn register_object(
        &mut self,
        id: u32,
        kind: WlObjectKind,
        owner_pid: u32,
        surface_idx: u8,
    ) -> bool {
        let mut i = 0usize;
        while i < MAX_WAYLAND_OBJECTS {
            if self.objects[i].kind != WlObjectKind::Free
                && self.objects[i].id == id
                && self.objects[i].owner_pid == owner_pid
            {
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
        i = 0;
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

    fn register_pool(
        &mut self,
        obj_id: u32,
        owner_pid: u32,
        fd: i32,
        size: u32,
        local_addr: u64,
        mapped_len: u64,
    ) {
        let mut free_slot: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_SHM_POOLS {
            if self.pools[i].active
                && self.pools[i].obj_id == obj_id
                && self.pools[i].owner_pid == owner_pid
            {
                if self.pools[i].fd != fd || self.pools[i].local_addr != local_addr {
                    unmap_close_pool(&mut self.pools[i]);
                }
                self.pools[i] = ShmPoolInfo {
                    active: true,
                    obj_id,
                    owner_pid,
                    fd,
                    size,
                    local_addr,
                    mapped_len,
                };
                return;
            }
            if !self.pools[i].active && free_slot.is_none() {
                free_slot = Some(i);
            }
            i += 1;
        }
        if let Some(slot) = free_slot {
            self.pools[slot] = ShmPoolInfo {
                active: true,
                obj_id,
                owner_pid,
                fd,
                size,
                local_addr,
                mapped_len,
            };
        }
    }

    fn find_pool(&self, owner_pid: u32, obj_id: u32) -> Option<ShmPoolInfo> {
        let mut i = 0usize;
        while i < MAX_SHM_POOLS {
            if self.pools[i].active && self.pools[i].obj_id == obj_id && self.pools[i].owner_pid == owner_pid {
                return Some(self.pools[i]);
            }
            i += 1;
        }
        None
    }

    fn find_pool_mut(&mut self, owner_pid: u32, obj_id: u32) -> Option<&mut ShmPoolInfo> {
        let mut i = 0usize;
        while i < MAX_SHM_POOLS {
            if self.pools[i].active && self.pools[i].obj_id == obj_id && self.pools[i].owner_pid == owner_pid {
                return Some(&mut self.pools[i]);
            }
            i += 1;
        }
        None
    }

    fn register_buffer(
        &mut self,
        obj_id: u32,
        owner_pid: u32,
        pool_id: u32,
        offset: u32,
        width: u32,
        height: u32,
        stride: u32,
        encoding: ShmPixelEncoding,
    ) {
        let mut free_slot: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_WL_BUFFERS {
            if self.buffers[i].active
                && self.buffers[i].obj_id == obj_id
                && self.buffers[i].owner_pid == owner_pid
            {
                self.buffers[i] = WlBufferInfo {
                    active: true,
                    obj_id,
                    owner_pid,
                    pool_id,
                    offset,
                    width,
                    height,
                    stride,
                    encoding,
                };
                return;
            }
            if !self.buffers[i].active && free_slot.is_none() {
                free_slot = Some(i);
            }
            i += 1;
        }
        if let Some(slot) = free_slot {
            self.buffers[slot] = WlBufferInfo {
                active: true,
                obj_id,
                owner_pid,
                pool_id,
                offset,
                width,
                height,
                stride,
                encoding,
            };
        }
    }

    fn find_buffer(&self, owner_pid: u32, obj_id: u32) -> Option<WlBufferInfo> {
        let strict_owner = owner_pid >= CLIENT_OWNER_BASE;
        let mut fallback = None;
        let mut i = 0usize;
        while i < MAX_WL_BUFFERS {
            if self.buffers[i].active && self.buffers[i].obj_id == obj_id {
                if self.buffers[i].owner_pid == owner_pid {
                    return Some(self.buffers[i]);
                }
                if !strict_owner && self.buffers[i].owner_pid < CLIENT_OWNER_BASE && fallback.is_none() {
                    fallback = Some(self.buffers[i]);
                }
            }
            i += 1;
        }
        fallback
    }

    fn bump_z(&mut self) -> u8 {
        if self.next_z >= 240 {
            let mut i = 0usize;
            while i < MAX_SHM_SURFACES {
                if self.surfaces[i].active {
                    self.surfaces[i].z_order = if self.surfaces[i].focused { 2 } else { 1 };
                }
                i += 1;
            }
            self.next_z = 3;
        }
        let z = self.next_z;
        self.next_z = self.next_z.saturating_add(1);
        z
    }

    pub fn focus_surface(&mut self, idx: usize) {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active {
            return;
        }
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            self.surfaces[i].focused = i == idx;
            i += 1;
        }
        self.surfaces[idx].minimized = false;
        self.surfaces[idx].z_order = self.bump_z();
        self.focused_surface_idx = idx as u8;
    }

    pub fn minimize_surface(&mut self, idx: usize) {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active {
            return;
        }
        self.surfaces[idx].minimized = true;
        self.surfaces[idx].focused = false;
        let mut best_idx: Option<usize> = None;
        let mut best_z = 0u8;
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].committed && !self.surfaces[i].minimized {
                if best_idx.is_none() || self.surfaces[i].z_order >= best_z {
                    best_idx = Some(i);
                    best_z = self.surfaces[i].z_order;
                }
            }
            i += 1;
        }
        if let Some(next_i) = best_idx {
            self.surfaces[next_i].focused = true;
            self.focused_surface_idx = next_i as u8;
        }
    }

    pub fn toggle_maximize_surface(&mut self, idx: usize) {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active {
            return;
        }
        self.focus_surface(idx);
        self.surfaces[idx].maximized = !self.surfaces[idx].maximized;
    }

    pub fn move_surface(&mut self, idx: usize, x: i32, y: i32, fb: Framebuffer) {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active || !fb.is_present() {
            return;
        }
        let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H) as i32;
        self.surfaces[idx].maximized = false;
        let max_x = (fb.width as i32 - 120).max(0);
        let max_y = (desktop_h - desktop::WIN_TITLE_H as i32 - 20).max(0);
        self.surfaces[idx].x = x.clamp(0, max_x);
        self.surfaces[idx].y = y.clamp(0, max_y);
    }

    pub fn resize_surface(&mut self, idx: usize, new_w: u32, new_h: u32, fb: Framebuffer) {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active || !fb.is_present() {
            return;
        }
        if !self.surfaces[idx].is_terminal {
            return;
        }
        let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
        let sx = self.surfaces[idx].x.max(0) as u32;
        let sy = self.surfaces[idx].y.max(0) as u32;
        let max_w = fb.width.saturating_sub(sx).max(320);
        let max_h = desktop_h.saturating_sub(sy).max(180);
        self.surfaces[idx].maximized = false;
        self.surfaces[idx].width = new_w.clamp(320, max_w);
        self.surfaces[idx].height = new_h.clamp(180, max_h);
    }

    pub fn alloc_socket_surface(&mut self, owner_key: u32) -> Option<usize> {
        let mut i = 1usize;
        while i < MAX_SHM_SURFACES {
            if !self.surfaces[i].active {
                let id = self.next_surface_id;
                self.next_surface_id = self.next_surface_id.wrapping_add(1).max(2);
                let mut s = CompositorSurface::empty();
                s.active = true;
                s.surface_id = id;
                s.owner_pid = owner_key;
                s.x = 120 + ((i as i32 - 1) * 32);
                s.y = 76 + ((i as i32 - 1) * 28);
                s.z_order = self.bump_z();
                s.title[..10].copy_from_slice(b"ExoSurface");
                s.title_len = 10;
                self.surfaces[i] = s;
                return Some(i);
            }
            i += 1;
        }
        None
    }

    pub fn disconnect_client(&mut self, owner_key: u32) -> bool {
        let mut removed_visible = false;
        let mut was_focused = false;
        let mut i = 1usize;
        while i < MAX_WAYLAND_OBJECTS {
            if self.objects[i].kind != WlObjectKind::Free && self.objects[i].owner_pid == owner_key {
                self.objects[i] = WlObject::empty();
            }
            i += 1;
        }
        i = 0;
        while i < MAX_WL_BUFFERS {
            if self.buffers[i].active && self.buffers[i].owner_pid == owner_key {
                self.buffers[i] = WlBufferInfo::empty();
            }
            i += 1;
        }
        i = 0;
        while i < MAX_SHM_POOLS {
            if self.pools[i].active && self.pools[i].owner_pid == owner_key {
                unmap_close_pool(&mut self.pools[i]);
            }
            i += 1;
        }
        i = 1;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].owner_pid == owner_key && !self.surfaces[i].is_terminal {
                if self.surfaces[i].committed && !self.surfaces[i].minimized {
                    removed_visible = true;
                }
                if self.surfaces[i].focused {
                    was_focused = true;
                }
                self.surfaces[i] = CompositorSurface::empty();
            }
            i += 1;
        }
        if was_focused {
            self.focus_surface(0);
        }
        removed_visible
    }

    pub fn alloc_surface(&mut self, owner_pid: u32, width: u32, height: u32) -> Option<usize> {
        let mut i = 1usize;
        while i < MAX_SHM_SURFACES {
            if !self.surfaces[i].active {
                let id = self.next_surface_id;
                self.next_surface_id = self.next_surface_id.wrapping_add(1).max(2);
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
                            3,
                            0x22,
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
                s.buf_size_bytes = size_bytes;
                s.x = 120 + ((i as i32 - 1) * 32);
                s.y = 76 + ((i as i32 - 1) * 28);
                s.width = width;
                s.height = height;
                s.stride_bytes = stride_bytes;
                s.format = ShmPixelEncoding::Argb8888 as u32;
                s.z_order = self.bump_z();
                s.damaged = true;
                s.damage = syscall::DamageRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                };
                s.title[..10].copy_from_slice(b"ExoSurface");
                s.title_len = 10;
                self.surfaces[i] = s;
                return Some(i);
            }
            i += 1;
        }
        None
    }

    pub fn destroy_surface(&mut self, surface_id: u32, sender_pid: u32) -> bool {
        let mut i = 1usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].surface_id == surface_id {
                if sender_pid != 0 && sender_pid != 1 && self.surfaces[i].owner_pid != sender_pid {
                    return false;
                }
                let was_focused = self.surfaces[i].focused;
                self.surfaces[i] = CompositorSurface::empty();
                if was_focused {
                    self.focus_surface(0);
                }
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
                if sender_pid != 0
                    && sender_pid != 1
                    && self.surfaces[i].owner_pid != 0
                    && self.surfaces[i].owner_pid != sender_pid
                {
                    return None;
                }
                let first_commit = !self.surfaces[i].committed && !self.surfaces[i].is_terminal;
                self.surfaces[i].damaged = true;
                self.surfaces[i].committed = true;
                self.surfaces[i].damage = rect;
                if first_commit {
                    self.focus_surface(i);
                }

                let s = &self.surfaces[i];
                if !s.is_terminal && s.shm_handle != 0 && s.virt_addr != 0 && s.owner_pid > 1 {
                    let dmg_y = rect.y.min(s.height);
                    let dmg_h = if rect.height == 0 {
                        s.height
                    } else {
                        rect.height
                    }
                    .min(s.height.saturating_sub(dmg_y));
                    let stride = s.stride_bytes as u64;
                    let base_off = s.buffer_offset as u64;
                    let mut r = 0u32;
                    while r < dmg_h {
                        let row_offset = base_off.saturating_add(((dmg_y + r) as u64).saturating_mul(stride));
                        if row_offset.saturating_add(stride) <= s.buf_size_bytes {
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
                        }
                        r += 1;
                    }
                }

                self.composite_surface(i, fb);

                let y_start = self.surfaces[i].y.max(0) as u32;
                let y_end = (self.surfaces[i].y + self.surfaces[i].height as i32 + 34).max(0) as u32;
                return Some((y_start.min(fb.height), y_end.min(fb.height)));
            }
            i += 1;
        }
        None
    }

    pub fn active_surface_count(&self) -> u32 {
        let mut count = 0u32;
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active {
                count += 1;
            }
            i += 1;
        }
        count
    }

    pub fn surface_window_bounds(&self, idx: usize, fb: Framebuffer) -> Option<(u32, u32, u32, u32)> {
        if idx >= MAX_SHM_SURFACES || !self.surfaces[idx].active || !self.surfaces[idx].committed {
            return None;
        }
        if idx == 0 || self.surfaces[idx].is_terminal {
            return Some(self.terminal_window_bounds(fb));
        }
        let s = &self.surfaces[idx];
        let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
        let max_w = fb.width.saturating_sub(16);
        let max_h = desktop_h.saturating_sub(36);
        let content_w = s.width.min(max_w);
        let content_h = s.height.min(max_h);
        if content_w < 16 || content_h < 16 {
            return None;
        }
        let wx = if s.maximized {
            8u32
        } else {
            (s.x.max(0) as u32).min(fb.width.saturating_sub(content_w + 6))
        };
        let wy = if s.maximized {
            8u32
        } else {
            (s.y.max(0) as u32).min(desktop_h.saturating_sub(content_h + 30))
        };
        Some((wx, wy, content_w + 4, content_h + 28))
    }

    pub fn terminal_window_bounds(&self, fb: Framebuffer) -> (u32, u32, u32, u32) {
        let s = &self.surfaces[0];
        let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
        if s.maximized {
            let x = 8;
            let y = 8;
            let w = fb.width.saturating_sub(16);
            let h = desktop_h.saturating_sub(16);
            return (x, y, w, h);
        }
        let (init_x, init_y, init_w, init_h) = if fb.width >= 1280 && fb.height >= 720 {
            (
                desktop::WIN_LEFT_X,
                desktop::WIN_TOP_Y,
                fb.width.saturating_sub(desktop::WIN_LEFT_X + 36).min(1360),
                desktop_h.saturating_sub(desktop::WIN_TOP_Y + 32).min(920),
            )
        } else {
            let cx = 374.min(fb.width.saturating_sub(340));
            let cy = 140.min(desktop_h.saturating_sub(240));
            (
                cx,
                cy,
                fb.width.saturating_sub(cx + 14).max(320),
                desktop_h.saturating_sub(cy + 12).max(180),
            )
        };
        let req_w = if s.width == 0 { init_w } else { s.width.max(320) };
        let req_h = if s.height == 0 { init_h } else { s.height.max(180) };
        let x = if s.width == 0 {
            init_x
        } else {
            (s.x.max(0) as u32).min(fb.width.saturating_sub(160))
        };
        let y = if s.height == 0 {
            init_y
        } else {
            (s.y.max(0) as u32).min(desktop_h.saturating_sub(64))
        };
        let w = req_w.min(fb.width.saturating_sub(x).max(320));
        let h = req_h.min(desktop_h.saturating_sub(y).max(180));
        (x, y, w, h)
    }

    pub fn composite_surface(&self, s_idx: usize, fb: Framebuffer) {
        self.composite_surface_inner(s_idx, fb, false);
    }

    pub fn composite_all_surfaces(&self, fb: Framebuffer) {
        if !fb.is_present() {
            return;
        }
        // Sort active visible windows by `z_order` ascending (painter algorithm)
        let mut order = [0usize; MAX_SHM_SURFACES];
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            let s = &self.surfaces[i];
            if s.active && s.committed && !s.minimized {
                order[count] = i;
                count += 1;
            }
            i += 1;
        }
        let mut a = 0usize;
        while a < count {
            let mut b = a + 1;
            while b < count {
                let za = (self.surfaces[order[a]].z_order as u16) + if self.surfaces[order[a]].focused { 256 } else { 0 };
                let zb = (self.surfaces[order[b]].z_order as u16) + if self.surfaces[order[b]].focused { 256 } else { 0 };
                if zb < za {
                    let tmp = order[a];
                    order[a] = order[b];
                    order[b] = tmp;
                }
                b += 1;
            }
            a += 1;
        }
        let mut k = 0usize;
        while k < count {
            self.composite_surface_inner(order[k], fb, true);
            k += 1;
        }
    }

    fn composite_surface_inner(&self, s_idx: usize, fb: Framebuffer, fill_term_interior: bool) {
        if s_idx >= MAX_SHM_SURFACES
            || !self.surfaces[s_idx].active
            || !self.surfaces[s_idx].committed
            || self.surfaces[s_idx].minimized
        {
            return;
        }
        let s = &self.surfaces[s_idx];
        if s.is_terminal {
            let (wx, wy, ww, wh) = self.terminal_window_bounds(fb);
            desktop::render_terminal_window_at(
                fb,
                wx,
                wy,
                ww,
                wh,
                s.title_str(),
                s.surface_id,
                s.focused,
                fill_term_interior,
            );
            return;
        }

        if s.virt_addr == 0 || s.width == 0 || s.height == 0 {
            return;
        }
        let base = fb.draw_target_base();
        if base == 0 {
            return;
        }

        let Some((wx, wy, ww, wh)) = self.surface_window_bounds(s_idx, fb) else {
            return;
        };
        let content_w = ww.saturating_sub(4);
        let content_h = wh.saturating_sub(28);
        desktop::render_wayland_window_chrome(
            fb,
            wx,
            wy,
            content_w,
            content_h,
            s.title_str(),
            s.surface_id,
            s.focused,
        );

        let bpp = fb.bytes_per_pixel() as usize;
        let dst_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let src_stride = s.stride_bytes as usize;
        let encoding = ShmPixelEncoding::from_wl_shm_format(s.format);
        let src_bpp = encoding.bytes_per_pixel() as usize;
        let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
        let is_bgr = fb.format == 1;

        let content_x = wx + 2;
        let content_y = wy + 26;
        let clip_w = content_w.min(fb.width.saturating_sub(content_x));
        let clip_h = content_h.min(desktop_h.saturating_sub(content_y));
        let src_base = (s.virt_addr as usize).saturating_add(s.buffer_offset as usize);

        let mut sy = 0u32;
        while sy < clip_h {
            let dy = content_y + sy;
            let row_src = src_base.saturating_add((sy as usize).saturating_mul(src_stride));
            let row_dst = base
                .saturating_add((dy as usize).saturating_mul(dst_stride))
                .saturating_add((content_x as usize).saturating_mul(bpp));

            if (row_src as u64).saturating_sub(s.virt_addr) + (clip_w as u64) * (src_bpp as u64)
                > s.buf_size_bytes.max((s.height as u64) * (src_stride as u64))
            {
                break;
            }

            let mut col = 0usize;
            while col < clip_w as usize {
                let (r, g, b, a) = unsafe {
                    match encoding {
                        ShmPixelEncoding::Argb8888 => {
                            let p = core::ptr::read((row_src + col * 4) as *const u32);
                            ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF, (p >> 24) & 0xFF)
                        }
                        ShmPixelEncoding::Xrgb8888 => {
                            let p = core::ptr::read((row_src + col * 4) as *const u32);
                            ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF, 255)
                        }
                        ShmPixelEncoding::Abgr8888 => {
                            let p = core::ptr::read((row_src + col * 4) as *const u32);
                            (p & 0xFF, (p >> 8) & 0xFF, (p >> 16) & 0xFF, (p >> 24) & 0xFF)
                        }
                        ShmPixelEncoding::Xbgr8888 => {
                            let p = core::ptr::read((row_src + col * 4) as *const u32);
                            (p & 0xFF, (p >> 8) & 0xFF, (p >> 16) & 0xFF, 255)
                        }
                        ShmPixelEncoding::Rgb565 => {
                            let v = core::ptr::read((row_src + col * 2) as *const u16) as u32;
                            let r5 = (v >> 11) & 0x1F;
                            let g6 = (v >> 5) & 0x3F;
                            let b5 = v & 0x1F;
                            ((r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2), 255)
                        }
                        ShmPixelEncoding::Rgb888 => {
                            let p = (row_src + col * 3) as *const u8;
                            (
                                core::ptr::read(p) as u32,
                                core::ptr::read(p.add(1)) as u32,
                                core::ptr::read(p.add(2)) as u32,
                                255,
                            )
                        }
                        ShmPixelEncoding::Bgr888 => {
                            let p = (row_src + col * 3) as *const u8;
                            (
                                core::ptr::read(p.add(2)) as u32,
                                core::ptr::read(p.add(1)) as u32,
                                core::ptr::read(p) as u32,
                                255,
                            )
                        }
                    }
                };
                if a != 0 {
                    let dst_px = if a >= 255 {
                        0
                    } else if bpp == 4 {
                        unsafe { core::ptr::read((row_dst + col * 4) as *const u32) }
                    } else {
                        unsafe {
                            let p = (row_dst + col * 3) as *const u8;
                            u32::from_le_bytes([
                                core::ptr::read(p),
                                core::ptr::read(p.add(1)),
                                core::ptr::read(p.add(2)),
                                0,
                            ])
                        }
                    };
                    let out_px = desktop::blend_rgba_to_fb(dst_px, r, g, b, a, is_bgr);
                    desktop::write_shadow_px(row_dst, col as u32, bpp, out_px);
                }
                col += 1;
            }
            sy += 1;
        }
    }

    pub fn handle_pointer_click(&mut self, mx: u32, my: u32, fb: Framebuffer) -> Option<(u32, u32)> {
        match self.handle_pointer_press(mx, my, fb) {
            UiAction::Redraw(y0, y1) => Some((y0, y1)),
            _ => None,
        }
    }

    /// Full interactive desktop & window hit-test for mouse press.
    pub fn handle_pointer_press(&mut self, mx: u32, my: u32, fb: Framebuffer) -> UiAction {
        if !fb.is_present() {
            return UiAction::None;
        }
        self.ensure_terminal_bounds(fb);
        let dock_y = fb.height.saturating_sub(desktop::DOCK_BAR_H);

        // 1. Bottom Plasma panel
        if my >= dock_y {
            if mx >= 4 && mx < 42 {
                self.kickoff_open = !self.kickoff_open;
                return UiAction::Redraw(0, fb.height);
            }
            if mx >= 44 && mx < 74 {
                if self.surfaces[0].minimized || !self.surfaces[0].focused {
                    self.focus_surface(0);
                } else {
                    self.minimize_surface(0);
                }
                return UiAction::Redraw(0, fb.height);
            }
            // Taskbar Window Pills (`idx = 0..MAX_SHM_SURFACES`)
            let clock_x = fb.width.saturating_sub(96);
            let tray_x = clock_x.saturating_sub(154);
            let mut pill_x = 180u32;
            let pill_w = 136u32;
            let mut i = 0usize;
            while i < MAX_SHM_SURFACES && pill_x + pill_w < tray_x {
                if self.surfaces[i].active && self.surfaces[i].committed {
                    if mx >= pill_x && mx < pill_x + pill_w {
                        if self.surfaces[i].minimized || !self.surfaces[i].focused {
                            self.focus_surface(i);
                        } else {
                            self.minimize_surface(i);
                        }
                        return UiAction::Redraw(0, fb.height);
                    }
                    pill_x = pill_x.saturating_add(pill_w + 6);
                }
                i += 1;
            }
            return UiAction::None;
        }

        // 2. Kickoff Application Launcher Menu (drawn on top!)
        if self.kickoff_open {
            let (menu_x, menu_y, menu_w, menu_h, sidebar_w, cat_pitch, row_h) =
                desktop::kickoff_layout(fb);
            let menu_bottom = menu_y + menu_h;
            let header_h = 42u32;
            let footer_h = 38u32;
            let body_y = menu_y + header_h + 1;
            let footer_y = menu_bottom.saturating_sub(footer_h + 1);

            if mx >= menu_x && mx < menu_x + menu_w && my >= menu_y && my < menu_bottom {
                // Sidebar categories
                let cat_y0 = body_y + desktop::KICKOFF_CAT_Y_OFFSET;
                if mx >= menu_x + 4 && mx < menu_x + sidebar_w && my >= cat_y0 && my < footer_y {
                    let cat_idx = (my - cat_y0) / cat_pitch;
                    if cat_idx < desktop::KICKOFF_CATEGORIES.len() as u32 {
                        self.selected_category = cat_idx as u8;
                        self.selected_app = 0;
                        return UiAction::Redraw(menu_y, fb.height);
                    }
                }

                // Application entries
                if mx >= menu_x + sidebar_w && my >= body_y + 20 && my < footer_y {
                    let items = desktop::kickoff_items_for_category(self.selected_category);
                    let row_pitch = row_h + 2;
                    let idx = ((my - (body_y + 20)) / row_pitch) as usize;
                    if idx < items.len() {
                        self.selected_app = idx as u8;
                        let action = items[idx].action;
                        if action != desktop::KickoffAction::CloseMenu {
                            self.kickoff_open = false;
                        } else {
                            self.kickoff_open = false;
                        }
                        return UiAction::Kickoff(action);
                    }
                }

                // Footer action buttons
                if my >= footer_y && my < menu_bottom {
                    if mx < menu_x + 86 {
                        self.selected_category = 1;
                        self.selected_app = 0;
                        return UiAction::Redraw(menu_y, fb.height);
                    } else if mx < menu_x + 174 {
                        self.kickoff_open = false;
                        return UiAction::Kickoff(desktop::KickoffAction::OpenTerminal);
                    } else if mx >= menu_x + menu_w - 190 && mx < menu_x + menu_w - 122 {
                        return UiAction::Kickoff(desktop::KickoffAction::ToggleOverlay(
                            desktop::DockOverlay::Calendar,
                        ));
                    } else if mx >= menu_x + menu_w - 122 && mx < menu_x + menu_w - 66 {
                        self.kickoff_open = false;
                        return UiAction::Kickoff(desktop::KickoffAction::Reboot);
                    } else if mx >= menu_x + menu_w - 66 {
                        self.kickoff_open = false;
                        return UiAction::Kickoff(desktop::KickoffAction::Shutdown);
                    }
                }
                return UiAction::None;
            }
        }

        // 3. Check active floating toplevel windows from frontmost z-order downward
        let mut order = [0usize; MAX_SHM_SURFACES];
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].committed && !self.surfaces[i].minimized {
                order[count] = i;
                count += 1;
            }
            i += 1;
        }
        let mut a = 0usize;
        while a < count {
            let mut b = a + 1;
            while b < count {
                let za = (self.surfaces[order[a]].z_order as u16)
                    + if self.surfaces[order[a]].focused { 256 } else { 0 };
                let zb = (self.surfaces[order[b]].z_order as u16)
                    + if self.surfaces[order[b]].focused { 256 } else { 0 };
                if zb > za {
                    let tmp = order[a];
                    order[a] = order[b];
                    order[b] = tmp;
                }
                b += 1;
            }
            a += 1;
        }

        let mut k = 0usize;
        while k < count {
            let s_idx = order[k];
            if let Some((wx, wy, ww, wh)) = self.surface_window_bounds(s_idx, fb) {
                if mx >= wx && mx < wx + ww && my >= wy && my < wy + wh {
                    let was_kickoff = self.kickoff_open;
                    self.kickoff_open = false;

                    // Titlebar controls & drag
                    let title_h = if self.surfaces[s_idx].is_terminal {
                        desktop::WIN_TITLE_H
                    } else {
                        26
                    };
                    if my < wy + title_h {
                        // Traffic light dots (x+6..21 = Close, x+21..35 = Minimize, x+35..49 = Maximize)
                        if my >= wy + 4 && my <= wy + 21 {
                            if mx >= wx + 6 && mx < wx + 21 {
                                if s_idx == 0 {
                                    self.minimize_surface(0);
                                } else {
                                    let s_id = self.surfaces[s_idx].surface_id;
                                    self.destroy_surface(s_id, 0);
                                }
                                return UiAction::Redraw(0, fb.height);
                            }
                            if mx >= wx + 21 && mx < wx + 35 {
                                self.minimize_surface(s_idx);
                                return UiAction::Redraw(0, fb.height);
                            }
                            if mx >= wx + 35 && mx < wx + 49 {
                                self.toggle_maximize_surface(s_idx);
                                return UiAction::Redraw(0, fb.height);
                            }
                        }

                        // Grab titlebar to move!
                        self.focus_surface(s_idx);
                        if self.surfaces[s_idx].maximized {
                            self.surfaces[s_idx].maximized = false;
                        }
                        let (cur_wx, cur_wy, _, _) =
                            self.surface_window_bounds(s_idx, fb).unwrap_or((wx, wy, ww, wh));
                        return UiAction::StartDrag {
                            surf_idx: s_idx,
                            off_x: mx as i32 - cur_wx as i32,
                            off_y: my as i32 - cur_wy as i32,
                        };
                    }

                    // Bottom-right corner resize grip for terminal window
                    if self.surfaces[s_idx].is_terminal
                        && mx >= wx + ww.saturating_sub(18)
                        && my >= wy + wh.saturating_sub(18)
                    {
                        self.focus_surface(s_idx);
                        return UiAction::StartResize {
                            surf_idx: s_idx,
                            start_mx: mx,
                            start_my: my,
                            orig_w: ww,
                            orig_h: wh,
                        };
                    }

                    self.focus_surface(s_idx);
                    if self.surfaces[s_idx].is_terminal {
                        if was_kickoff {
                            // Returning terminal click clears Kickoff and also allows selection immediately
                            return UiAction::StartTermSelect { surf_idx: s_idx };
                        }
                        return UiAction::StartTermSelect { surf_idx: s_idx };
                    }

                    let sx = (mx - wx).saturating_sub(2) as i32;
                    let sy = (my - wy).saturating_sub(26) as i32;
                    self.last_pointer = PointerEventRecord {
                        valid: true,
                        surface_idx: s_idx as u8,
                        surface_id: self.surfaces[s_idx].surface_id,
                        sx,
                        sy,
                        button: 0x110, // BTN_LEFT
                        pressed: true,
                    };
                    return UiAction::ClientPointer {
                        surf_idx: s_idx,
                        sx,
                        sy,
                    };
                }
            }
            k += 1;
        }

        if self.kickoff_open {
            self.kickoff_open = false;
            return UiAction::Redraw(0, fb.height);
        }

        UiAction::None
    }

    pub fn dispatch_wire(
        &mut self,
        sender_pid: u32,
        bytes: &[u8],
        fb: Framebuffer,
    ) -> (i64, Option<(u32, u32)>) {
        let mut empty_fds = WlFdQueue::empty();
        let (_consumed, processed, dirty) =
            self.dispatch_wire_inner(sender_pid, bytes, &mut empty_fds, None, fb);
        (processed, dirty)
    }

    pub fn dispatch_wire_stream(
        &mut self,
        owner_key: u32,
        bytes: &[u8],
        rx_fds: &mut WlFdQueue,
        tx_buf: &mut WlTxBuf,
        fb: Framebuffer,
    ) -> (usize, i64, Option<(u32, u32)>) {
        self.dispatch_wire_inner(owner_key, bytes, rx_fds, Some(tx_buf), fb)
    }

    fn dispatch_wire_inner(
        &mut self,
        sender_pid: u32,
        bytes: &[u8],
        rx_fds: &mut WlFdQueue,
        mut tx_buf: Option<&mut WlTxBuf>,
        fb: Framebuffer,
    ) -> (usize, i64, Option<(u32, u32)>) {
        let mut offset = 0usize;
        let mut processed = 0i64;
        let mut dirty_min = u32::MAX;
        let mut dirty_max = 0u32;
        while offset + 8 <= bytes.len() {
            let object_id = read_u32(bytes, offset);
            let size_op = read_u32(bytes, offset + 4);
            let raw_size = (size_op >> 16) as usize;
            if raw_size < 8 || (raw_size & 3) != 0 || raw_size > 4096 {
                return (usize::MAX, processed, None);
            }
            let opcode = (size_op & 0xFFFF) as u16;
            if offset + raw_size > bytes.len() {
                break;
            }
            let payload = &bytes[offset + 8..offset + raw_size];
            if let Some((y0, y1)) = self.handle_single_wire_msg(
                sender_pid,
                object_id,
                opcode,
                payload,
                rx_fds,
                tx_buf.as_deref_mut(),
                fb,
            ) {
                if y0 < dirty_min {
                    dirty_min = y0;
                }
                if y1 > dirty_max {
                    dirty_max = y1;
                }
            }
            offset += raw_size;
            processed += 1;
            self.wire_messages_processed = self.wire_messages_processed.saturating_add(1);
        }
        let dirty_bounds = if dirty_min < dirty_max {
            Some((dirty_min, dirty_max))
        } else {
            None
        };
        (offset, processed, dirty_bounds)
    }

    fn next_serial(&mut self) -> u32 {
        let s = self.next_configure_serial.max(1);
        self.next_configure_serial = s.wrapping_add(1).max(1);
        s
    }

    fn handle_single_wire_msg(
        &mut self,
        sender_pid: u32,
        object_id: u32,
        opcode: u16,
        payload: &[u8],
        rx_fds: &mut WlFdQueue,
        tx_buf: Option<&mut WlTxBuf>,
        fb: Framebuffer,
    ) -> Option<(u32, u32)> {
        let Some(obj_idx) = self.find_object(sender_pid, object_id) else {
            return None;
        };
        let kind = self.objects[obj_idx].kind;
        let surf_idx = self.objects[obj_idx].surface_idx;

        match (kind, opcode) {
            // wl_display.sync(new_id callback)
            (WlObjectKind::Display, 0) => {
                let cb_id = read_u32(payload, 0);
                if cb_id != 0 {
                    if let Some(tx) = tx_buf {
                        tx.push_u32_evt(cb_id, 0, 0);
                        tx.push_u32_evt(1, 1, cb_id);
                    } else {
                        let _ = self.register_object(cb_id, WlObjectKind::Callback, sender_pid, 0xFF);
                    }
                }
            }
            // wl_display.get_registry(new_id registry)
            (WlObjectKind::Display, 1) => {
                let reg_id = read_u32(payload, 0);
                if reg_id != 0 {
                    let _ = self.register_object(reg_id, WlObjectKind::Registry, sender_pid, 0xFF);
                    if let Some(tx) = tx_buf {
                        tx.push_registry_global(reg_id, 1, b"wl_compositor", 6);
                        tx.push_registry_global(reg_id, 2, b"wl_shm", 1);
                        tx.push_registry_global(reg_id, 3, b"xdg_wm_base", 6);
                        tx.push_registry_global(reg_id, 4, b"wl_seat", 7);
                    }
                }
            }
            // wl_registry.bind(name, [interface, version], new_id)
            (WlObjectKind::Registry, 0) => {
                let name = read_u32(payload, 0);
                let new_id = if payload.len() >= 8 {
                    read_u32(payload, payload.len() - 4)
                } else {
                    0
                };
                if new_id != 0 {
                    let iface = if payload.len() >= 16 {
                        parse_wl_string(&payload[4..])
                    } else {
                        &[]
                    };
                    let target_kind = if iface == b"wl_compositor" {
                        WlObjectKind::Compositor
                    } else if iface == b"wl_shm" {
                        WlObjectKind::Shm
                    } else if iface == b"xdg_wm_base" {
                        WlObjectKind::XdgWmBase
                    } else if iface == b"wl_seat" {
                        WlObjectKind::Seat
                    } else {
                        match name {
                            1 => WlObjectKind::Compositor,
                            2 => WlObjectKind::Shm,
                            3 => WlObjectKind::XdgWmBase,
                            4 => WlObjectKind::Seat,
                            _ => WlObjectKind::Compositor,
                        }
                    };
                    let _ = self.register_object(new_id, target_kind, sender_pid, 0xFF);
                    if let Some(tx) = tx_buf {
                        match target_kind {
                            WlObjectKind::Shm => {
                                tx.push_u32_evt(new_id, 0, 0); // WL_SHM_FORMAT_ARGB8888
                                tx.push_u32_evt(new_id, 0, 1); // WL_SHM_FORMAT_XRGB8888
                            }
                            WlObjectKind::Seat => {
                                tx.push_u32_evt(new_id, 0, 3); // pointer | keyboard
                                tx.push_str_evt(new_id, 1, b"seat0");
                            }
                            _ => {}
                        }
                    }
                }
            }
            // wl_compositor.create_surface(new_id surface)
            (WlObjectKind::Compositor, 0) => {
                let surf_id = read_u32(payload, 0);
                if surf_id != 0 {
                    if sender_pid < CLIENT_OWNER_BASE
                        && self.surfaces[0].active
                        && self.surfaces[0].is_terminal
                        && self.surfaces[0].owner_pid == 0
                    {
                        self.surfaces[0].owner_pid = sender_pid;
                        self.surfaces[0].damaged = true;
                        let _ = self.register_object(surf_id, WlObjectKind::Surface, sender_pid, 0);
                    } else if sender_pid >= CLIENT_OWNER_BASE {
                        if let Some(s_idx) = self.alloc_socket_surface(sender_pid) {
                            let _ = self.register_object(
                                surf_id,
                                WlObjectKind::Surface,
                                sender_pid,
                                s_idx as u8,
                            );
                        }
                    } else if let Some(s_idx) = self.alloc_surface(sender_pid, 360, 240) {
                        let _ = self.register_object(
                            surf_id,
                            WlObjectKind::Surface,
                            sender_pid,
                            s_idx as u8,
                        );
                    }
                }
            }
            // wl_compositor.create_region(new_id region)
            (WlObjectKind::Compositor, 1) => {
                let reg_id = read_u32(payload, 0);
                if reg_id != 0 {
                    let _ = self.register_object(reg_id, WlObjectKind::Region, sender_pid, 0xFF);
                }
            }
            // wl_compositor.release()
            (WlObjectKind::Compositor, 2) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // wl_region.destroy()
            (WlObjectKind::Region, 0) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // wl_shm.create_pool(new_id pool, [fd in SCM_RIGHTS or inline], size)
            (WlObjectKind::Shm, 0) => {
                let pool_id = read_u32(payload, 0);
                let (fd, size) = if let Some(sfd) = rx_fds.pop_front() {
                    let sz = if payload.len() >= 12 {
                        read_u32(payload, 8)
                    } else {
                        read_u32(payload, 4)
                    };
                    (sfd, sz)
                } else if payload.len() >= 12 {
                    (read_u32(payload, 4) as i32, read_u32(payload, 8))
                } else {
                    (-1, read_u32(payload, 4))
                };
                if pool_id != 0 {
                    let (local_addr, mapped_len) = map_shm_fd(fd, size);
                    let _ = self.register_object(pool_id, WlObjectKind::ShmPool, sender_pid, 0xFF);
                    self.register_pool(pool_id, sender_pid, fd, size, local_addr, mapped_len);
                } else if fd >= 0 && sender_pid >= CLIENT_OWNER_BASE {
                    unsafe {
                        let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                    }
                }
            }
            // wl_shm.release()
            (WlObjectKind::Shm, 1) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // wl_shm_pool.create_buffer(new_id buffer, offset, width, height, stride, format)
            (WlObjectKind::ShmPool, 0) => {
                let buf_id = read_u32(payload, 0);
                if buf_id != 0 {
                    let _ = self.register_object(buf_id, WlObjectKind::Buffer, sender_pid, 0xFF);
                    if payload.len() >= 24 {
                        let buf_offset = read_u32(payload, 4);
                        let width = read_u32(payload, 8);
                        let height = read_u32(payload, 12);
                        let stride = read_u32(payload, 16);
                        let format = read_u32(payload, 20);
                        let encoding = ShmPixelEncoding::from_wl_shm_format(format);
                        let min_stride = width.saturating_mul(encoding.bytes_per_pixel());
                        if (1..=1920).contains(&width) && (1..=1080).contains(&height) {
                            self.register_buffer(
                                buf_id,
                                sender_pid,
                                object_id,
                                buf_offset,
                                width,
                                height,
                                stride.max(min_stride),
                                encoding,
                            );
                        }
                    }
                }
            }
            // wl_shm_pool.destroy()
            (WlObjectKind::ShmPool, 1) => {
                if let Some(p) = self.find_pool_mut(sender_pid, object_id) {
                    let old_addr = p.local_addr;
                    unmap_close_pool(p);
                    if old_addr != 0 {
                        let mut si = 1usize;
                        while si < MAX_SHM_SURFACES {
                            if self.surfaces[si].active && self.surfaces[si].virt_addr == old_addr {
                                self.surfaces[si].virt_addr = 0;
                                self.surfaces[si].buf_size_bytes = 0;
                            }
                            si += 1;
                        }
                    }
                }
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // wl_shm_pool.resize(size)
            (WlObjectKind::ShmPool, 2) => {
                let new_sz = read_u32(payload, 0);
                let mut remapped_addr: Option<(u64, u32)> = None;
                if let Some(p) = self.find_pool_mut(sender_pid, object_id) {
                    if new_sz as u64 > p.mapped_len && p.fd >= 0 {
                        if p.local_addr != 0 && p.mapped_len != 0 {
                            unsafe {
                                let _ = syscall::syscall2(syscall::SYS_MUNMAP, p.local_addr, p.mapped_len);
                            }
                        }
                        let (new_addr, new_map_len) = map_shm_fd(p.fd, new_sz);
                        p.local_addr = new_addr;
                        p.mapped_len = new_map_len;
                        remapped_addr = Some((new_addr, new_sz));
                    }
                    p.size = new_sz;
                }
                if let Some((new_addr, sz)) = remapped_addr {
                    let mut si = 1usize;
                    while si < MAX_SHM_SURFACES {
                        let buf_id = self.surfaces[si].attached_buffer;
                        if self.surfaces[si].active
                            && self.surfaces[si].owner_pid == sender_pid
                            && buf_id != 0
                        {
                            if let Some(b) = self.find_buffer(sender_pid, buf_id) {
                                if b.pool_id == object_id {
                                    self.surfaces[si].virt_addr = new_addr;
                                    self.surfaces[si].buf_size_bytes = sz as u64;
                                }
                            }
                        }
                        si += 1;
                    }
                }
            }
            // wl_buffer.destroy()
            (WlObjectKind::Buffer, 0) => {
                let mut i = 0usize;
                while i < MAX_WL_BUFFERS {
                    if self.buffers[i].active
                        && self.buffers[i].obj_id == object_id
                        && self.buffers[i].owner_pid == sender_pid
                    {
                        self.buffers[i].active = false;
                    }
                    i += 1;
                }
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // wl_surface.destroy()
            (WlObjectKind::Surface, 0) => {
                let mut redraw = false;
                if (surf_idx as usize) < MAX_SHM_SURFACES && !self.surfaces[surf_idx as usize].is_terminal {
                    redraw = self.surfaces[surf_idx as usize].committed;
                    let was_focused = self.surfaces[surf_idx as usize].focused;
                    self.surfaces[surf_idx as usize] = CompositorSurface::empty();
                    if was_focused {
                        self.focus_surface(0);
                    }
                }
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
                if redraw {
                    return Some((0, fb.height));
                }
            }
            // wl_surface.attach(buffer_id, x, y)
            (WlObjectKind::Surface, 1) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let buf_id = read_u32(payload, 0);
                    let dx = if payload.len() >= 8 { read_u32(payload, 4) as i32 } else { 0 };
                    let dy = if payload.len() >= 12 { read_u32(payload, 8) as i32 } else { 0 };
                    if buf_id == 0 {
                        self.surfaces[surf_idx as usize].attached_buffer = 0;
                        self.surfaces[surf_idx as usize].committed = false;
                    } else {
                        let buf_info = self.find_buffer(sender_pid, buf_id);
                        let pool_info = buf_info.and_then(|b| self.find_pool(sender_pid, b.pool_id));
                        let s = &mut self.surfaces[surf_idx as usize];
                        s.attached_buffer = buf_id;
                        if dx != 0 || dy != 0 {
                            s.x = s.x.saturating_add(dx);
                            s.y = s.y.saturating_add(dy);
                        }
                        if let Some(b) = buf_info {
                            if !s.is_terminal {
                                if let Some(pool) = pool_info {
                                    if pool.local_addr != 0 {
                                        s.virt_addr = pool.local_addr;
                                        s.buf_size_bytes = pool.size as u64;
                                    }
                                }
                                s.buffer_offset = b.offset;
                                s.width = b.width;
                                s.height = b.height;
                                s.stride_bytes = b.stride;
                                s.format = b.encoding as u32;
                            }
                        }
                        s.damaged = true;
                    }
                }
            }
            // wl_surface.damage / damage_buffer(x, y, w, h)
            (WlObjectKind::Surface, 2 | 9) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES && payload.len() >= 16 {
                    let s = &mut self.surfaces[surf_idx as usize];
                    let x = read_u32(payload, 0);
                    let y = read_u32(payload, 4);
                    let w = read_u32(payload, 8);
                    let h = read_u32(payload, 12);
                    if s.damage.width == 0 || s.damage.height == 0 {
                        s.damage = syscall::DamageRect {
                            x,
                            y,
                            width: w,
                            height: h,
                        };
                    } else {
                        let nx0 = s.damage.x.min(x);
                        let ny0 = s.damage.y.min(y);
                        let nx1 = s.damage.x.saturating_add(s.damage.width).max(x.saturating_add(w));
                        let ny1 = s.damage.y.saturating_add(s.damage.height).max(y.saturating_add(h));
                        s.damage = syscall::DamageRect {
                            x: nx0,
                            y: ny0,
                            width: nx1.saturating_sub(nx0),
                            height: ny1.saturating_sub(ny0),
                        };
                    }
                }
            }
            // wl_surface.frame(new_id callback)
            (WlObjectKind::Surface, 3) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let cb_id = read_u32(payload, 0);
                    if cb_id != 0 {
                        self.surfaces[surf_idx as usize].frame_callback_id = cb_id;
                    }
                }
            }
            // wl_surface.commit()
            (WlObjectKind::Surface, 6) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let si = surf_idx as usize;
                    let is_term = self.surfaces[si].is_terminal;
                    let attached_buf = self.surfaces[si].attached_buffer;
                    let has_drawable = is_term
                        || (attached_buf != 0
                            && self.surfaces[si].virt_addr != 0
                            && self.surfaces[si].width > 0
                            && self.surfaces[si].height > 0);
                    let need_initial_configure = self.surfaces[si].toplevel
                        && self.surfaces[si].xdg_surface_id != 0
                        && !self.surfaces[si].configure_sent;
                    let xdg_surf_id = self.surfaces[si].xdg_surface_id;
                    let xdg_top_id = self.surfaces[si].xdg_toplevel_id;
                    let frame_cb = self.surfaces[si].frame_callback_id;
                    self.surfaces[si].frame_callback_id = 0;

                    let res = if has_drawable {
                        let s_id = self.surfaces[si].surface_id;
                        let rect = self.surfaces[si].damage;
                        let out = self.commit_surface_damage(s_id, sender_pid, rect, fb);
                        self.surfaces[si].damage = syscall::DamageRect {
                            x: 0,
                            y: 0,
                            width: 0,
                            height: 0,
                        };
                        out
                    } else {
                        None
                    };

                    if let Some(tx) = tx_buf {
                        if !is_term && attached_buf != 0 {
                            tx.push_empty_evt(attached_buf, 0); // wl_buffer.release
                        }
                        if need_initial_configure {
                            let serial = self.next_serial();
                            self.surfaces[si].configure_sent = true;
                            if xdg_top_id != 0 {
                                tx.push_xdg_toplevel_configure(xdg_top_id, 0, 0);
                            }
                            tx.push_u32_evt(xdg_surf_id, 0, serial); // xdg_surface.configure
                        }
                        if frame_cb != 0 {
                            tx.push_u32_evt(frame_cb, 0, 1); // wl_callback.done
                            tx.push_u32_evt(1, 1, frame_cb); // wl_display.delete_id
                        }
                    } else if need_initial_configure {
                        self.surfaces[si].configure_sent = true;
                    }
                    return res;
                }
            }
            // xdg_wm_base.destroy()
            (WlObjectKind::XdgWmBase, 0) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // xdg_wm_base.get_xdg_surface(new_id, wl_surface)
            (WlObjectKind::XdgWmBase, 2) => {
                let xdg_surf_id = read_u32(payload, 0);
                let wl_surf_id = read_u32(payload, 4);
                let s_idx = self
                    .find_object(sender_pid, wl_surf_id)
                    .map(|idx| self.objects[idx].surface_idx)
                    .unwrap_or(0xFF);
                if xdg_surf_id != 0 {
                    let _ = self.register_object(
                        xdg_surf_id,
                        WlObjectKind::XdgSurface,
                        sender_pid,
                        s_idx,
                    );
                    if (s_idx as usize) < MAX_SHM_SURFACES {
                        self.surfaces[s_idx as usize].xdg_surface_id = xdg_surf_id;
                    }
                }
            }
            // xdg_surface.destroy()
            (WlObjectKind::XdgSurface, 0) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    self.surfaces[surf_idx as usize].xdg_surface_id = 0;
                    self.surfaces[surf_idx as usize].configure_sent = false;
                }
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            // xdg_surface.get_toplevel(new_id)
            (WlObjectKind::XdgSurface, 1) => {
                let top_id = read_u32(payload, 0);
                if top_id != 0 {
                    let _ = self.register_object(
                        top_id,
                        WlObjectKind::XdgToplevel,
                        sender_pid,
                        surf_idx,
                    );
                    if (surf_idx as usize) < MAX_SHM_SURFACES {
                        self.surfaces[surf_idx as usize].toplevel = true;
                        self.surfaces[surf_idx as usize].xdg_toplevel_id = top_id;
                    }
                }
            }
            // xdg_surface.ack_configure(serial)
            (WlObjectKind::XdgSurface, 4) => {}
            // xdg_toplevel.destroy()
            (WlObjectKind::XdgToplevel, 0) => {
                let mut redraw = false;
                if (surf_idx as usize) < MAX_SHM_SURFACES
                    && !self.surfaces[surf_idx as usize].is_terminal
                {
                    redraw = self.surfaces[surf_idx as usize].committed;
                    let was_focused = self.surfaces[surf_idx as usize].focused;
                    self.surfaces[surf_idx as usize] = CompositorSurface::empty();
                    if was_focused {
                        self.focus_surface(0);
                    }
                }
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
                if redraw {
                    return Some((0, fb.height));
                }
            }
            // xdg_toplevel.set_title(title: string)
            (WlObjectKind::XdgToplevel, 2) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let text = parse_wl_string(payload);
                    if !text.is_empty() {
                        let dst = &mut self.surfaces[surf_idx as usize];
                        let n = text.len().min(dst.title.len());
                        dst.title.fill(0);
                        dst.title[..n].copy_from_slice(&text[..n]);
                        dst.title_len = n as u8;
                        dst.toplevel = true;
                    }
                }
            }
            // xdg_toplevel.set_app_id(app_id: string)
            (WlObjectKind::XdgToplevel, 3) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    let text = parse_wl_string(payload);
                    if !text.is_empty() {
                        let dst = &mut self.surfaces[surf_idx as usize];
                        let n = text.len().min(dst.app_id.len());
                        dst.app_id.fill(0);
                        dst.app_id[..n].copy_from_slice(&text[..n]);
                        dst.app_id_len = n as u8;
                        dst.toplevel = true;
                    }
                }
            }
            // xdg_toplevel.set_maximized()
            (WlObjectKind::XdgToplevel, 9) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    self.surfaces[surf_idx as usize].maximized = true;
                    self.surfaces[surf_idx as usize].minimized = false;
                    return Some((0, fb.height));
                }
            }
            // xdg_toplevel.unset_maximized()
            (WlObjectKind::XdgToplevel, 10) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    self.surfaces[surf_idx as usize].maximized = false;
                    return Some((0, fb.height));
                }
            }
            // xdg_toplevel.set_minimized()
            (WlObjectKind::XdgToplevel, 12 | 13) => {
                if (surf_idx as usize) < MAX_SHM_SURFACES {
                    self.minimize_surface(surf_idx as usize);
                    return Some((0, fb.height));
                }
            }
            // wl_seat.get_pointer(new_id) / get_keyboard(new_id) / release()
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
            (WlObjectKind::Seat, 3) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            (WlObjectKind::Pointer, 1) | (WlObjectKind::Keyboard, 0) => {
                self.objects[obj_idx].kind = WlObjectKind::Free;
                if let Some(tx) = tx_buf {
                    tx.push_u32_evt(1, 1, object_id);
                }
            }
            _ => {}
        }
        None
    }
}

pub struct WlFdQueue {
    fds: [i32; WL_CLIENT_FD_CAP],
    head: usize,
    len: usize,
}

impl WlFdQueue {
    pub const fn empty() -> Self {
        Self {
            fds: [-1; WL_CLIENT_FD_CAP],
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, fd: i32) {
        if fd < 0 {
            return;
        }
        if self.len >= WL_CLIENT_FD_CAP {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
            }
            return;
        }
        let idx = (self.head + self.len) % WL_CLIENT_FD_CAP;
        self.fds[idx] = fd;
        self.len += 1;
    }

    pub fn pop_front(&mut self) -> Option<i32> {
        if self.len == 0 {
            return None;
        }
        let fd = self.fds[self.head];
        self.fds[self.head] = -1;
        self.head = (self.head + 1) % WL_CLIENT_FD_CAP;
        self.len -= 1;
        Some(fd)
    }

    pub fn close_all(&mut self) {
        while let Some(fd) = self.pop_front() {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
            }
        }
    }
}

pub struct WlTxBuf {
    pub data: [u8; WL_CLIENT_TX_CAP],
    pub len: usize,
}

impl WlTxBuf {
    pub const fn new() -> Self {
        Self {
            data: [0; WL_CLIENT_TX_CAP],
            len: 0,
        }
    }

    fn push_raw(&mut self, obj_id: u32, opcode: u16, words: &[u32]) {
        let total = 8 + words.len() * 4;
        if self.len + total > self.data.len() {
            return;
        }
        let hdr = ((total as u32) << 16) | (opcode as u32);
        self.data[self.len..self.len + 4].copy_from_slice(&obj_id.to_le_bytes());
        self.data[self.len + 4..self.len + 8].copy_from_slice(&hdr.to_le_bytes());
        self.len += 8;
        let mut i = 0usize;
        while i < words.len() {
            self.data[self.len..self.len + 4].copy_from_slice(&words[i].to_le_bytes());
            self.len += 4;
            i += 1;
        }
    }

    pub fn push_empty_evt(&mut self, obj_id: u32, opcode: u16) {
        self.push_raw(obj_id, opcode, &[]);
    }

    pub fn push_u32_evt(&mut self, obj_id: u32, opcode: u16, val: u32) {
        self.push_raw(obj_id, opcode, &[val]);
    }

    pub fn push_xdg_toplevel_configure(&mut self, obj_id: u32, width: i32, height: i32) {
        self.push_raw(obj_id, 0, &[width as u32, height as u32, 0]);
    }

    fn push_str_bytes(&mut self, text: &[u8]) {
        let len_nul = text.len() + 1;
        let padded = (len_nul + 3) & !3;
        self.data[self.len..self.len + 4].copy_from_slice(&(len_nul as u32).to_le_bytes());
        self.len += 4;
        self.data[self.len..self.len + text.len()].copy_from_slice(text);
        self.data[self.len + text.len()..self.len + padded].fill(0);
        self.len += padded;
    }

    pub fn push_str_evt(&mut self, obj_id: u32, opcode: u16, text: &[u8]) {
        let padded = ((text.len() + 1) + 3) & !3;
        let total = 8 + 4 + padded;
        if self.len + total > self.data.len() {
            return;
        }
        let hdr = ((total as u32) << 16) | (opcode as u32);
        self.data[self.len..self.len + 4].copy_from_slice(&obj_id.to_le_bytes());
        self.data[self.len + 4..self.len + 8].copy_from_slice(&hdr.to_le_bytes());
        self.len += 8;
        self.push_str_bytes(text);
    }

    pub fn push_registry_global(&mut self, reg_id: u32, name: u32, iface: &[u8], version: u32) {
        let padded = ((iface.len() + 1) + 3) & !3;
        let total = 8 + 4 + 4 + padded + 4;
        if self.len + total > self.data.len() {
            return;
        }
        let hdr = (total as u32) << 16; // opcode 0 = global
        self.data[self.len..self.len + 4].copy_from_slice(&reg_id.to_le_bytes());
        self.data[self.len + 4..self.len + 8].copy_from_slice(&hdr.to_le_bytes());
        self.data[self.len + 8..self.len + 12].copy_from_slice(&name.to_le_bytes());
        self.len += 12;
        self.push_str_bytes(iface);
        self.data[self.len..self.len + 4].copy_from_slice(&version.to_le_bytes());
        self.len += 4;
    }
}

fn map_shm_fd(fd: i32, size: u32) -> (u64, u64) {
    if fd < 0 || size == 0 {
        return (0, 0);
    }
    let capped = (size as u64).min(64 * 1024 * 1024).max(4096);
    let aligned = (capped + 4095) & !4095;
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
        (rc as u64, aligned)
    } else {
        (0, 0)
    }
}

fn unmap_close_pool(p: &mut ShmPoolInfo) {
    if p.local_addr != 0 && p.mapped_len != 0 {
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_MUNMAP, p.local_addr, p.mapped_len);
        }
    }
    if p.fd >= 0 {
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p.fd as u64);
        }
    }
    *p = ShmPoolInfo::empty();
}

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

pub struct WlClientSlot {
    pub fd: i32,
    pub owner_key: u32,
    pub rx_buf: [u8; WL_CLIENT_RX_CAP],
    pub rx_len: usize,
    pub rx_fds: WlFdQueue,
    pub tx_buf: WlTxBuf,
}

impl WlClientSlot {
    pub const fn empty() -> Self {
        Self {
            fd: -1,
            owner_key: 0,
            rx_buf: [0; WL_CLIENT_RX_CAP],
            rx_len: 0,
            rx_fds: WlFdQueue::empty(),
            tx_buf: WlTxBuf::new(),
        }
    }

    fn close_fd_and_reset(&mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, self.fd as u64);
            }
        }
        self.fd = -1;
        self.rx_len = 0;
        self.rx_fds.close_all();
        self.tx_buf.len = 0;
    }
}

pub struct SocketPollOutcome {
    pub did_io: bool,
    pub full_repaint: bool,
    pub dirty: Option<(u32, u32)>,
}

pub struct WaylandSocketServer {
    pub listener_fd: i32,
    pub bind_cooldown: u32,
    pub announced: bool,
    pub clients: [WlClientSlot; MAX_WAYLAND_CLIENTS],
}

struct StaticWaylandSocket(core::cell::UnsafeCell<WaylandSocketServer>);

unsafe impl Sync for StaticWaylandSocket {}

static WAYLAND_SOCKET_SERVER: StaticWaylandSocket =
    StaticWaylandSocket(core::cell::UnsafeCell::new(WaylandSocketServer::new()));

/// Retourne l'instance `.bss` de `WaylandSocketServer` sans allouer ~36 Kio sur la pile.
pub fn wayland_socket_mut() -> &'static mut WaylandSocketServer {
    // SAFETY: `fb_server` exécute sa boucle principale sur un thread unique.
    unsafe { &mut *WAYLAND_SOCKET_SERVER.0.get() }
}

impl WaylandSocketServer {
    pub const fn new() -> Self {
        Self {
            listener_fd: -1,
            bind_cooldown: 0,
            announced: false,
            clients: [const { WlClientSlot::empty() }; MAX_WAYLAND_CLIENTS],
        }
    }

    fn try_bind(&mut self) {
        if self.listener_fd >= 0 {
            return;
        }
        if self.bind_cooldown > 0 {
            self.bind_cooldown -= 1;
            return;
        }
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_MKDIR, b"/run\0".as_ptr() as u64, 0o755);
            let _ = syscall::syscall1(syscall::SYS_UNLINK, b"/run/wayland-0\0".as_ptr() as u64);
        }
        let fd = unsafe {
            syscall::syscall3(
                syscall::SYS_SOCKET,
                1, // AF_UNIX
                1 | syscall::O_NONBLOCK | syscall::O_CLOEXEC, // SOCK_STREAM
                0,
            )
        };
        if fd < 0 {
            self.bind_cooldown = 15;
            return;
        }
        let mut sa = SockAddrUn {
            sun_family: 1,
            sun_path: [0; 108],
        };
        let path = b"/run/wayland-0\0";
        sa.sun_path[..path.len()].copy_from_slice(path);
        let bind_rc = unsafe {
            syscall::syscall3(
                syscall::SYS_BIND,
                fd as u64,
                &sa as *const SockAddrUn as u64,
                core::mem::size_of::<SockAddrUn>() as u64,
            )
        };
        if bind_rc < 0 {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
            }
            self.bind_cooldown = 15;
            return;
        }
        let listen_rc = unsafe { syscall::syscall2(syscall::SYS_LISTEN, fd as u64, 16) };
        if listen_rc < 0 {
            unsafe {
                let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
            }
            self.bind_cooldown = 15;
            return;
        }
        self.listener_fd = fd as i32;
        if !self.announced {
            self.announced = true;
            let msg = b"fb_server: wayland socket /run/wayland-0 ready\n";
            unsafe {
                let _ = syscall::syscall3(syscall::SYS_EXO_LOG, msg.as_ptr() as u64, msg.len() as u64, 1);
            }
        }
    }

    pub fn poll(&mut self, state: &mut WaylandState, fb: Framebuffer) -> SocketPollOutcome {
        const MSG_DONTWAIT: u64 = 0x40;
        const EINTR_ERR: i64 = -4;

        let mut outcome = SocketPollOutcome {
            did_io: false,
            full_repaint: false,
            dirty: None,
        };

        self.try_bind();
        if self.listener_fd < 0 {
            return outcome;
        }

        // Accept up to 4 newly connected clients per pass.
        let mut accepts = 0usize;
        while accepts < 4 {
            let cfd = unsafe { syscall::syscall3(syscall::SYS_ACCEPT, self.listener_fd as u64, 0, 0) };
            if cfd < 0 {
                break;
            }
            outcome.did_io = true;
            let mut placed = false;
            let mut si = 0usize;
            while si < MAX_WAYLAND_CLIENTS {
                if self.clients[si].fd < 0 {
                    self.clients[si].fd = cfd as i32;
                    self.clients[si].owner_key = CLIENT_OWNER_BASE | (si as u32);
                    self.clients[si].rx_len = 0;
                    self.clients[si].rx_fds.close_all();
                    self.clients[si].tx_buf.len = 0;
                    placed = true;
                    break;
                }
                si += 1;
            }
            if !placed {
                unsafe {
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, cfd as u64);
                }
                break;
            }
            accepts += 1;
        }

        let mut dirty_min = u32::MAX;
        let mut dirty_max = 0u32;
        let mut si = 0usize;
        while si < MAX_WAYLAND_CLIENTS {
            if self.clients[si].fd < 0 {
                si += 1;
                continue;
            }
            let owner_key = self.clients[si].owner_key;
            let mut disconnected = false;
            let mut reads = 0usize;
            while reads < 4 && self.clients[si].rx_len < WL_CLIENT_RX_CAP {
                let start = self.clients[si].rx_len;
                let cap = WL_CLIENT_RX_CAP - start;
                let mut iov = RawIoVec {
                    iov_base: self.clients[si].rx_buf[start..].as_mut_ptr() as u64,
                    iov_len: cap as u64,
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
                        self.clients[si].fd as u64,
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
                    let cmsg = unsafe { core::ptr::read_unaligned(ctrl.as_ptr() as *const UnixCmsghdr) };
                    if cmsg.cmsg_level == 1 && cmsg.cmsg_type == 1 && cmsg.cmsg_len >= 16 {
                        let avail_bytes = (msg.msg_controllen as usize).min(ctrl.len()).min(cmsg.cmsg_len);
                        let n_fds = (avail_bytes.saturating_sub(16)) / 4;
                        let mut fi = 0usize;
                        while fi < n_fds {
                            let off = 16 + fi * 4;
                            let rfd = i32::from_le_bytes([
                                ctrl[off],
                                ctrl[off + 1],
                                ctrl[off + 2],
                                ctrl[off + 3],
                            ]);
                            self.clients[si].rx_fds.push(rfd);
                            fi += 1;
                        }
                    }
                }
                if rc == 0 && msg.msg_controllen == 0 {
                    disconnected = true;
                    break;
                }
                self.clients[si].rx_len += rc as usize;
                outcome.did_io = true;
                reads += 1;
            }

            if !disconnected && self.clients[si].rx_len >= 8 {
                let slot = &mut self.clients[si];
                let (consumed, msgs, slot_dirty) = state.dispatch_wire_stream(
                    owner_key,
                    &slot.rx_buf[..slot.rx_len],
                    &mut slot.rx_fds,
                    &mut slot.tx_buf,
                    fb,
                );
                if consumed == usize::MAX {
                    disconnected = true;
                } else {
                    if consumed > 0 {
                        if consumed < slot.rx_len {
                            slot.rx_buf.copy_within(consumed..slot.rx_len, 0);
                        }
                        slot.rx_len -= consumed;
                    }
                    if msgs > 0 {
                        outcome.did_io = true;
                    }
                    if let Some((y0, y1)) = slot_dirty {
                        if y0 < dirty_min {
                            dirty_min = y0;
                        }
                        if y1 > dirty_max {
                            dirty_max = y1;
                        }
                    }
                }
            }

            if !disconnected && self.clients[si].tx_buf.len > 0 {
                let tx_len = self.clients[si].tx_buf.len;
                let iov = RawIoVec {
                    iov_base: self.clients[si].tx_buf.data.as_ptr() as u64,
                    iov_len: tx_len as u64,
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
                        self.clients[si].fd as u64,
                        &msg as *const UnixMsghdr as u64,
                        MSG_DONTWAIT,
                    )
                };
                if sent > 0 {
                    let s_usize = (sent as usize).min(tx_len);
                    if s_usize < tx_len {
                        self.clients[si].tx_buf.data.copy_within(s_usize..tx_len, 0);
                    }
                    self.clients[si].tx_buf.len -= s_usize;
                    outcome.did_io = true;
                } else if sent < 0 && sent != syscall::EAGAIN && sent != EINTR_ERR {
                    disconnected = true;
                }
            }

            if disconnected {
                self.clients[si].close_fd_and_reset();
                if state.disconnect_client(owner_key) {
                    outcome.full_repaint = true;
                }
                outcome.did_io = true;
            }
            si += 1;
        }

        if dirty_min < dirty_max {
            outcome.dirty = Some((dirty_min, dirty_max));
        }
        outcome
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

#[inline]
fn parse_wl_string(payload: &[u8]) -> &[u8] {
    if payload.len() < 4 {
        return &[];
    }
    let len = read_u32(payload, 0) as usize;
    if len == 0 || 4 + len > payload.len() {
        return &[];
    }
    let mut slice = &payload[4..4 + len];
    while let Some(&last) = slice.last() {
        if last == 0 {
            slice = &slice[..slice.len() - 1];
        } else {
            break;
        }
    }
    slice
}
