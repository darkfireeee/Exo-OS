#![allow(dead_code)]

//! Strate 1 `SurfaceGrant` / `DamageRect` scanout table for `fb_server` (S5B).
//!
//! All Wayland wire protocol parsing, `/run/wayland-0` socket I/O, and client SHM pool
//! validation live exclusively in Strate 2 (`servers/exo_compositor`). This module owns only:
//! - Typed `SurfaceGrant` allocation & damage scanout (`DISPLAY_OP_CREATE_SURFACE`,
//!   `DISPLAY_OP_DESTROY_SURFACE`, `DISPLAY_OP_COMMIT_DAMAGE`, `DISPLAY_OP_FLIP`).
//! - Z-ordered compositing of committed surface grants + emergency terminal bounds.

use core::cell::UnsafeCell;

use crate::desktop;
use crate::framebuffer::Framebuffer;
use exo_syscall_abi as syscall;

pub const MAX_SHM_SURFACES: usize = 8;

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
            0 | 0x34325241 => Self::Argb8888,
            1 | 0x34325258 => Self::Xrgb8888,
            2 | 0x34324241 => Self::Abgr8888,
            3 | 0x34324258 => Self::Xbgr8888,
            4 | 0x36314752 => Self::Rgb565,
            5 | 0x34324752 => Self::Rgb888,
            6 | 0x34324742 => Self::Bgr888,
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
    pub toplevel: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub focused: bool,
    pub is_terminal: bool,
    pub is_root_compositor: bool,
    pub committed: bool,
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
            toplevel: false,
            minimized: false,
            maximized: false,
            focused: false,
            is_terminal: false,
            is_root_compositor: false,
            committed: false,
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

pub struct SurfaceTable {
    pub surfaces: [CompositorSurface; MAX_SHM_SURFACES],
    pub last_pointer: PointerEventRecord,
    pub next_surface_id: u32,
    pub next_z: u8,
    pub wire_messages_processed: u64,
    pub kickoff_open: bool,
    pub selected_category: u8,
    pub selected_app: u8,
    pub focused_surface_idx: u8,
}

impl SurfaceTable {
    pub const fn new() -> Self {
        let mut surfaces = [CompositorSurface::empty(); MAX_SHM_SURFACES];
        let mut term = CompositorSurface::empty();
        term.active = true;
        term.surface_id = 1;
        term.owner_pid = 0;
        term.x = desktop::WIN_LEFT_X as i32;
        term.y = desktop::WIN_TOP_Y as i32;
        term.z_order = 2;
        term.toplevel = true;
        // Terminal does NOT auto-open on boot — user launches it as a normal app.
        term.minimized = true;
        term.focused = false;
        term.is_terminal = true;
        term.committed = true;
        term.title = init_bytes_32(b"ExoTerm - exosh (Wayland)");
        term.title_len = 25;
        surfaces[0] = term;

        Self {
            surfaces,
            last_pointer: PointerEventRecord::empty(),
            next_surface_id: 2,
            next_z: 3,
            wire_messages_processed: 0,
            kickoff_open: false,
            selected_category: 0,
            selected_app: 0,
            focused_surface_idx: 0,
        }
    }

    /// Returns true while Strate 2 `exo_compositor` owns a committed fullscreen root surface.
    #[inline]
    pub fn has_active_root_compositor(&self) -> bool {
        let mut i = 1usize;
        while i < MAX_SHM_SURFACES {
            let s = &self.surfaces[i];
            if s.active && s.is_root_compositor && s.committed {
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn ensure_terminal_bounds(&mut self, fb: Framebuffer) {
        if !fb.is_present() {
            return;
        }
        let s = &mut self.surfaces[0];
        if s.width == 0 || s.height == 0 {
            let desktop_h = fb.height.saturating_sub(desktop::DOCK_BAR_H);
            if fb.width >= 1280 && fb.height >= 720 {
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
            if self.surfaces[i].active && self.surfaces[i].committed && !self.surfaces[i].minimized
            {
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

    pub fn alloc_root_surface(&mut self, owner_pid: u32, width: u32, height: u32) -> Option<usize> {
        // Reclaim any stale root compositor surface from a previous Strate 2 respawn.
        let mut r = 1usize;
        while r < MAX_SHM_SURFACES {
            if self.surfaces[r].active && self.surfaces[r].is_root_compositor {
                self.surfaces[r] = CompositorSurface::empty();
            }
            r += 1;
        }
        let idx = self.alloc_surface(owner_pid, width, height)?;
        let s = &mut self.surfaces[idx];
        s.is_root_compositor = true;
        s.x = 0;
        s.y = 0;
        s.toplevel = false;
        s.title = init_bytes_32(b"Exo-OS Strata Desktop");
        s.title_len = 21;
        syscall::boot_log(b"fb_server: root compositor surface granted\n");
        Some(idx)
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
                        3,
                        0x22,
                        u64::MAX,
                        0,
                    )
                };
                let virt_addr = if virt_rc > 0 { virt_rc as u64 } else { 0 };
                if virt_addr == 0 {
                    return None;
                }
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
                    } else {
                        let _ = unsafe {
                            syscall::syscall2(syscall::SYS_MUNMAP, virt_addr, size_bytes)
                        };
                        return None;
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
    ) -> Result<(u32, u32), i64> {
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].surface_id == surface_id {
                // FIX-12: Protect both client surfaces AND surface_id=1 (owner_pid == 0)
                // against unauthorized third-party damage commits.
                let authorized = if self.surfaces[i].owner_pid == 0 {
                    sender_pid == 0
                        || syscall::peer_is_any_service(
                            sender_pid,
                            &[
                                syscall::SVC_INIT_SERVER,
                                syscall::SVC_TTY_SERVER,
                                syscall::SVC_FB_SERVER,
                                syscall::SVC_EXO_COMPOSITOR,
                                syscall::SVC_EXOSH,
                            ],
                        )
                } else {
                    sender_pid == 0
                        || sender_pid == 1
                        || self.surfaces[i].owner_pid == sender_pid
                };
                if !authorized {
                    return Err(syscall::EACCES);
                }
                let first_commit = !self.surfaces[i].committed && !self.surfaces[i].is_terminal;
                self.surfaces[i].damaged = true;
                self.surfaces[i].committed = true;
                self.surfaces[i].damage = rect;
                if first_commit {
                    self.focus_surface(i);
                }

                let s = &self.surfaces[i];
                let is_root = s.is_root_compositor;
                let dmg_x = rect.x.min(s.width);
                let dmg_y = rect.y.min(s.height);
                let dmg_w = if rect.width == 0 {
                    s.width.saturating_sub(dmg_x)
                } else {
                    rect.width.min(s.width.saturating_sub(dmg_x))
                };
                let dmg_h = if rect.height == 0 {
                    s.height.saturating_sub(dmg_y)
                } else {
                    rect.height.min(s.height.saturating_sub(dmg_y))
                };
                let direct_shadow_copy = is_root
                    && fb.is_present()
                    && fb.shadow_addr != 0
                    && fb.bytes_per_pixel() == 4
                    && fb.format != 1
                    && fb.stride_pixels == s.width
                    && dmg_x == 0
                    && dmg_w == s.width
                    && s.buffer_offset == 0;
                if !s.is_terminal && s.shm_handle != 0 && s.virt_addr != 0 && s.owner_pid > 1 && dmg_h > 0 {
                    let stride = s.stride_bytes as u64;
                    let base_off = (s.buffer_offset as u64)
                        .saturating_add((dmg_y as u64).saturating_mul(stride));
                    let band_bytes = (dmg_h as u64).saturating_mul(stride);
                    if base_off.saturating_add(band_bytes) <= s.buf_size_bytes {
                        let remote_src = s.shm_handle.saturating_add(base_off);
                        let local_dst = if direct_shadow_copy {
                            fb.shadow_addr.saturating_add(base_off)
                        } else {
                            s.virt_addr.saturating_add(base_off)
                        };
                        let copy_rc = unsafe {
                            syscall::syscall6(
                                syscall::SYS_EXO_MEM_COPY_FROM_PID,
                                s.owner_pid as u64,
                                remote_src,
                                local_dst,
                                band_bytes,
                                0,
                                0,
                            )
                        };
                        if copy_rc == syscall::ENOENT || copy_rc == syscall::ESRCH {
                            self.surfaces[i] = CompositorSurface::empty();
                            return Err(syscall::ENOENT);
                        }
                    }
                }

                if is_root {
                    if !direct_shadow_copy {
                        self.blit_root_compositor_rect(i, fb, dmg_x, dmg_y, dmg_w, dmg_h);
                    }
                    return Ok((dmg_y.min(fb.height), dmg_y.saturating_add(dmg_h).min(fb.height)));
                }

                self.composite_surface(i, fb);

                let y_start = self.surfaces[i].y.max(0) as u32;
                let y_end =
                    (self.surfaces[i].y + self.surfaces[i].height as i32 + 34).max(0) as u32;
                return Ok((y_start.min(fb.height), y_end.min(fb.height)));
            }
            i += 1;
        }
        Err(syscall::ENOENT)
    }

    /// Ultra-fast borderless scanout of the Strate 2 root desktop compositor surface (`0xAARRGGBB`).
    fn blit_root_compositor_rect(
        &self,
        s_idx: usize,
        fb: Framebuffer,
        dmg_x: u32,
        dmg_y: u32,
        dmg_w: u32,
        dmg_h: u32,
    ) {
        if !fb.is_present() || s_idx >= MAX_SHM_SURFACES {
            return;
        }
        let s = &self.surfaces[s_idx];
        if !s.active || s.virt_addr == 0 || dmg_w == 0 || dmg_h == 0 {
            return;
        }
        let base = fb.draw_target_base();
        if base == 0 {
            return;
        }
        let bpp = fb.bytes_per_pixel() as usize;
        let dst_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let src_stride = s.stride_bytes as usize;
        let x0 = dmg_x.min(fb.width).min(s.width) as usize;
        let y0 = dmg_y.min(fb.height).min(s.height) as usize;
        let x1 = dmg_x.saturating_add(dmg_w).min(fb.width).min(s.width) as usize;
        let y1 = dmg_y.saturating_add(dmg_h).min(fb.height).min(s.height) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let is_bgr = fb.format == 1;
        let copy_px = x1 - x0;

        let mut row = y0;
        while row < y1 {
            let src_off = (row as u64).saturating_mul(src_stride as u64)
                .saturating_add((x0 as u64).saturating_mul(4));
            if src_off.saturating_add((copy_px as u64).saturating_mul(4)) > s.buf_size_bytes {
                break;
            }
            let src_ptr = (s.virt_addr.saturating_add(src_off)) as *const u32;
            let dst_row_addr = base
                .saturating_add(row.saturating_mul(dst_stride))
                .saturating_add(x0.saturating_mul(bpp));

            if bpp == 4 && !is_bgr {
                unsafe {
                    core::ptr::copy_nonoverlapping(src_ptr, dst_row_addr as *mut u32, copy_px);
                }
            } else if bpp == 4 {
                let dst_ptr = dst_row_addr as *mut u32;
                let mut col = 0usize;
                while col < copy_px {
                    unsafe {
                        let p = core::ptr::read(src_ptr.add(col));
                        let r = (p >> 16) & 0xFF;
                        let g = (p >> 8) & 0xFF;
                        let b = p & 0xFF;
                        core::ptr::write(dst_ptr.add(col), (b << 16) | (g << 8) | r);
                    }
                    col += 1;
                }
            } else if bpp == 3 {
                let dst_ptr = dst_row_addr as *mut u8;
                let mut col = 0usize;
                while col < copy_px {
                    unsafe {
                        let p = core::ptr::read(src_ptr.add(col));
                        let r = ((p >> 16) & 0xFF) as u8;
                        let g = ((p >> 8) & 0xFF) as u8;
                        let b = (p & 0xFF) as u8;
                        let o = col * 3;
                        if is_bgr {
                            core::ptr::write(dst_ptr.add(o), r);
                            core::ptr::write(dst_ptr.add(o + 1), g);
                            core::ptr::write(dst_ptr.add(o + 2), b);
                        } else {
                            core::ptr::write(dst_ptr.add(o), b);
                            core::ptr::write(dst_ptr.add(o + 1), g);
                            core::ptr::write(dst_ptr.add(o + 2), r);
                        }
                    }
                    col += 1;
                }
            }
            row += 1;
        }
    }

    /// Lightweight compatibility handler for `exosh` boot registration over `DISPLAY_OP_WAYLAND_WIRE`.
    /// Real Wayland wire parsing lives exclusively in Strate 2 (`servers/exo_compositor`).
    pub fn acknowledge_terminal_wire(&mut self, sender_pid: u32, byte_len: usize) -> i64 {
        if byte_len < 8 {
            return 0;
        }
        self.surfaces[0].owner_pid = sender_pid;
        self.surfaces[0].committed = true;
        self.wire_messages_processed = self.wire_messages_processed.saturating_add(1);
        1
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
            return (8, 8, fb.width.saturating_sub(16), desktop_h.saturating_sub(16));
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
                let za = (self.surfaces[order[a]].z_order as u16)
                    + if self.surfaces[order[a]].focused { 256 } else { 0 };
                let zb = (self.surfaces[order[b]].z_order as u16)
                    + if self.surfaces[order[b]].focused { 256 } else { 0 };
                if zb < za {
                    order.swap(a, b);
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

        let mut sy = 0u32;
        while sy < clip_h {
            let dy = content_y + sy;
            let Some(row_stride_off) = (sy as u64).checked_mul(src_stride as u64) else {
                break;
            };
            let Some(row_rel_off) = (s.buffer_offset as u64).checked_add(row_stride_off) else {
                break;
            };
            let Some(row_bytes) = (clip_w as u64).checked_mul(src_bpp as u64) else {
                break;
            };
            let Some(row_end) = row_rel_off.checked_add(row_bytes) else {
                break;
            };
            if row_end > s.buf_size_bytes {
                break;
            }
            let Some(row_src_u64) = s.virt_addr.checked_add(row_rel_off) else {
                break;
            };
            let row_src = row_src_u64 as usize;
            let row_dst = base
                .saturating_add((dy as usize).saturating_mul(dst_stride))
                .saturating_add((content_x as usize).saturating_mul(bpp));

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
                            (
                                (r5 << 3) | (r5 >> 2),
                                (g6 << 2) | (g6 >> 4),
                                (b5 << 3) | (b5 >> 2),
                                255,
                            )
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

    pub fn handle_pointer_press(&mut self, mx: u32, my: u32, fb: Framebuffer) -> UiAction {
        if !fb.is_present() {
            return UiAction::None;
        }
        self.ensure_terminal_bounds(fb);
        let dock_y = fb.height.saturating_sub(desktop::DOCK_BAR_H);

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

        if self.kickoff_open {
            let (menu_x, menu_y, menu_w, menu_h, sidebar_w, cat_pitch, row_h) =
                desktop::kickoff_layout(fb);
            let menu_bottom = menu_y + menu_h;
            let header_h = 42u32;
            let footer_h = 38u32;
            let body_y = menu_y + header_h + 1;
            let footer_y = menu_bottom.saturating_sub(footer_h + 1);

            if mx >= menu_x && mx < menu_x + menu_w && my >= menu_y && my < menu_bottom {
                let cat_y0 = body_y + desktop::KICKOFF_CAT_Y_OFFSET;
                if mx >= menu_x + 4 && mx < menu_x + sidebar_w && my >= cat_y0 && my < footer_y {
                    let cat_idx = (my - cat_y0) / cat_pitch;
                    if cat_idx < desktop::KICKOFF_CATEGORIES.len() as u32 {
                        self.selected_category = cat_idx as u8;
                        self.selected_app = 0;
                        return UiAction::Redraw(menu_y, fb.height);
                    }
                }
                if mx >= menu_x + sidebar_w && my >= body_y + 20 && my < footer_y {
                    let items = desktop::kickoff_items_for_category(self.selected_category);
                    let row_pitch = row_h + 2;
                    let idx = ((my - (body_y + 20)) / row_pitch) as usize;
                    if idx < items.len() {
                        self.selected_app = idx as u8;
                        let action = items[idx].action;
                        self.kickoff_open = false;
                        return UiAction::Kickoff(action);
                    }
                }
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

        let mut order = [0usize; MAX_SHM_SURFACES];
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_SHM_SURFACES {
            if self.surfaces[i].active && self.surfaces[i].committed && !self.surfaces[i].minimized
            {
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
                    order.swap(a, b);
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
                    self.kickoff_open = false;
                    let title_h = if self.surfaces[s_idx].is_terminal {
                        desktop::WIN_TITLE_H
                    } else {
                        26
                    };
                    if my < wy + title_h {
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
                        button: 0x110,
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
}

struct SurfaceTableCell(UnsafeCell<SurfaceTable>);
unsafe impl Sync for SurfaceTableCell {}

static SURFACES: SurfaceTableCell = SurfaceTableCell(UnsafeCell::new(SurfaceTable::new()));

pub fn surface_table_mut() -> &'static mut SurfaceTable {
    unsafe { &mut *SURFACES.0.get() }
}
