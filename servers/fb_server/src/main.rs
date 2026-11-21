#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(dead_code, unused_imports, unused_variables, unused_mut)]

#[cfg(target_os = "none")]
use core::cell::UnsafeCell;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use exo_syscall_abi as syscall;

#[cfg(target_os = "none")]
#[path = "../../../exo-boot/src/display/font.rs"]
pub mod shared_font;
#[cfg(target_os = "none")]
mod desktop;
#[cfg(target_os = "none")]
mod wayland;

#[cfg(target_os = "none")]
use shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};
#[cfg(target_os = "none")]
use desktop::{DesktopState, DockOverlay, DOCK_BAR_H, HEADER_TOTAL_H, KickoffAction, TERM_LEFT_X, WIN_TITLE_H};
#[cfg(target_os = "none")]
use wayland::{UiAction, WaylandState};

#[cfg(target_os = "none")]
const CHAR_W: u32 = FONT_GLYPH_WIDTH as u32;
#[cfg(target_os = "none")]
const CHAR_H: u32 = FONT_GLYPH_HEIGHT as u32;
#[cfg(target_os = "none")]
const TERM_DEFAULT_FG: (u8, u8, u8) = (0xe6, 0xf1, 0xff);
#[cfg(target_os = "none")]
const TERM_DEFAULT_BG: (u8, u8, u8) = (0x12, 0x14, 0x1d);
#[cfg(target_os = "none")]
const TERM_MAX_COLS: usize = 256;
#[cfg(target_os = "none")]
const TERM_MAX_ROWS: usize = 72;

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct TermCell {
    ch: u8,
    fg: [u8; 3],
    bg: [u8; 3],
}

#[cfg(target_os = "none")]
impl TermCell {
    const fn empty() -> Self {
        Self {
            ch: b' ',
            fg: [TERM_DEFAULT_FG.0, TERM_DEFAULT_FG.1, TERM_DEFAULT_FG.2],
            bg: [TERM_DEFAULT_BG.0, TERM_DEFAULT_BG.1, TERM_DEFAULT_BG.2],
        }
    }
}
// Doit rester PAIR car bit 0 = IPC_FLAG_TIMEOUT (0x0001).
#[cfg(target_os = "none")]
const RECV_TIMEOUT_MS: u64 = 16;
#[cfg(target_os = "none")]
const BATCH_DRAIN_MAX: usize = 64;

#[cfg(target_os = "none")]
const ANSI_GROUND: u8 = 0;
#[cfg(target_os = "none")]
const ANSI_ESC: u8 = 1;
#[cfg(target_os = "none")]
const ANSI_CSI: u8 = 2;

#[cfg(target_os = "none")]
#[inline]
fn fill_buffer_rect(
    base: usize,
    row_stride: usize,
    x: u32,
    y: u32,
    x_end: u32,
    y_end: u32,
    bpp: usize,
    pixel: u32,
) {
    if base == 0 || x >= x_end || y >= y_end {
        return;
    }
    if bpp == 4 {
        let width = (x_end - x) as usize;
        let pixel64 = (pixel as u64) | ((pixel as u64) << 32);
        let mut row = y;
        while row < y_end {
            let row_base = base
                .saturating_add((row as usize).saturating_mul(row_stride))
                .saturating_add((x as usize).saturating_mul(4));
            let mut col = 0usize;
            while col < width {
                let addr = row_base + col * 4;
                unsafe {
                    if col + 1 < width && (addr & 7) == 0 {
                        core::ptr::write(addr as *mut u64, pixel64);
                        col += 2;
                    } else {
                        core::ptr::write(addr as *mut u32, pixel);
                        col += 1;
                    }
                }
            }
            row = row.saturating_add(1);
        }
        return;
    }
    if bpp == 3 {
        let bytes = pixel.to_le_bytes();
        let width = (x_end - x) as usize;
        let mut row = y;
        while row < y_end {
            let row_base = base
                .saturating_add((row as usize).saturating_mul(row_stride))
                .saturating_add((x as usize).saturating_mul(3));
            let mut col = 0usize;
            while col < width {
                let addr = row_base.saturating_add(col.saturating_mul(3));
                unsafe {
                    core::ptr::write(addr as *mut u8, bytes[0]);
                    core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                }
                col += 1;
            }
            row = row.saturating_add(1);
        }
    }
}

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub struct Framebuffer {
    pub virt_addr: u64,
    pub shadow_addr: u64,
    pub width: u32,
    pub height: u32,
    pub stride_pixels: u32,
    pub bpp: u32,
    pub format: u32,
    pub size_bytes: u64,
}

#[cfg(target_os = "none")]
impl Framebuffer {
    const fn absent() -> Self {
        Self {
            virt_addr: 0,
            shadow_addr: 0,
            width: 0,
            height: 0,
            stride_pixels: 0,
            bpp: 0,
            format: 0,
            size_bytes: 0,
        }
    }

    #[inline]
    fn is_present(self) -> bool {
        self.virt_addr != 0
            && self.width != 0
            && self.height != 0
            && self.stride_pixels >= self.width
            && self.size_bytes != 0
            && self.bytes_per_pixel() >= 3
    }

    #[inline]
    fn draw_target_base(self) -> usize {
        if self.shadow_addr != 0 {
            self.shadow_addr as usize
        } else {
            self.virt_addr as usize
        }
    }

    #[inline]
    fn bytes_per_pixel(self) -> u32 {
        ((self.bpp.saturating_add(7)) / 8).max(1)
    }

    #[inline]
    fn encode(self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            1 => ((b as u32) << 16) | ((g as u32) << 8) | (r as u32),
            _ => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        }
    }

    #[inline]
    fn pixel_offset(self, x: u32, y: u32) -> Option<usize> {
        if !self.is_present() || x >= self.width || y >= self.height {
            return None;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let offset = (y as usize)
            .checked_mul(self.stride_pixels as usize)?
            .checked_add(x as usize)?
            .checked_mul(bpp)?;
        if offset.checked_add(bpp)? <= self.size_bytes as usize {
            Some(offset)
        } else {
            None
        }
    }

    /// Écrit un pixel directement dans le MMIO + Shadow (utilisé uniquement pour le petit curseur souris 12x18).
    #[inline]
    fn put_pixel_direct(self, x: u32, y: u32, pixel: u32) {
        let Some(offset) = self.pixel_offset(x, y) else {
            return;
        };
        let bpp = self.bytes_per_pixel();
        let bytes = pixel.to_le_bytes();
        if self.shadow_addr != 0 {
            let s_addr = (self.shadow_addr as usize).saturating_add(offset);
            unsafe {
                match bpp {
                    4 => core::ptr::write(s_addr as *mut u32, pixel),
                    3 => {
                        core::ptr::write(s_addr as *mut u8, bytes[0]);
                        core::ptr::write((s_addr + 1) as *mut u8, bytes[1]);
                        core::ptr::write((s_addr + 2) as *mut u8, bytes[2]);
                    }
                    _ => {}
                }
            }
        }
        let addr = (self.virt_addr as usize).saturating_add(offset);
        unsafe {
            match bpp {
                4 => core::ptr::write_volatile(addr as *mut u32, pixel),
                3 => {
                    core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                    core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                }
                _ => {}
            }
        }
    }

    #[inline]
    fn read_pixel(self, x: u32, y: u32) -> u32 {
        let Some(offset) = self.pixel_offset(x, y) else {
            return 0;
        };
        let base = self.draw_target_base();
        let addr = base.saturating_add(offset);
        unsafe {
            match self.bytes_per_pixel() {
                4 => core::ptr::read(addr as *const u32),
                3 => {
                    let b0 = core::ptr::read(addr as *const u8);
                    let b1 = core::ptr::read((addr + 1) as *const u8);
                    let b2 = core::ptr::read((addr + 2) as *const u8);
                    u32::from_le_bytes([b0, b1, b2, 0])
                }
                _ => 0,
            }
        }
    }

    /// Remplit un rectangle uniquement dans la cible de dessin (Shadow RAM si disponible).
    #[inline]
    fn fill_rect_shadow(self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
        if !self.is_present() || w == 0 || h == 0 {
            return;
        }
        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        fill_buffer_rect(
            self.draw_target_base(),
            row_stride,
            x,
            y,
            x_end,
            y_end,
            bpp,
            pixel,
        );
    }

    /// Copie les lignes modifiées (`y_min..y_max`) du Shadow Buffer RAM vers le MMIO en rafales 64 bits.
    fn flush_rows(self, y_min: u32, y_max: u32) {
        if !self.is_present() || self.shadow_addr == 0 || y_min >= y_max {
            return;
        }
        let y0 = y_min.min(self.height) as usize;
        let y1 = y_max.min(self.height) as usize;
        if y0 >= y1 {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let offset = y0.saturating_mul(row_stride);
        let len = (y1 - y0)
            .saturating_mul(row_stride)
            .min((self.size_bytes as usize).saturating_sub(offset));
        if len == 0 {
            return;
        }
        unsafe {
            let src = (self.shadow_addr as usize + offset) as *const u64;
            let dst = (self.virt_addr as usize + offset) as *mut u64;
            let qwords = len / 8;
            let mut i = 0usize;
            while i < qwords {
                core::ptr::write_volatile(dst.add(i), core::ptr::read(src.add(i)));
                i += 1;
            }
            let rem_start = qwords * 8;
            let src_b = (self.shadow_addr as usize + offset + rem_start) as *const u8;
            let dst_b = (self.virt_addr as usize + offset + rem_start) as *mut u8;
            let mut b = 0usize;
            while rem_start + b < len {
                core::ptr::write_volatile(dst_b.add(b), core::ptr::read(src_b.add(b)));
                b += 1;
            }
        }
    }

    /// Copie un rectangle précis (`x..x+w, y..y+h`) du Shadow Buffer RAM vers le MMIO.
    /// Ultra-rapide : pour le curseur souris (12x18), ne transfère que 864 octets au lieu de 73 Ko !
    fn flush_rect(self, x: u32, y: u32, w: u32, h: u32) {
        if !self.is_present() || self.shadow_addr == 0 || w == 0 || h == 0 {
            return;
        }
        let x0 = x.min(self.width) as usize;
        let y0 = y.min(self.height) as usize;
        let x1 = (x.saturating_add(w)).min(self.width) as usize;
        let y1 = (y.saturating_add(h)).min(self.height) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let copy_bytes = (x1 - x0).saturating_mul(bpp);
        let mut row_y = y0;
        while row_y < y1 {
            let offset = row_y.saturating_mul(stride).saturating_add(x0.saturating_mul(bpp));
            unsafe {
                let src = (self.shadow_addr as usize + offset) as *const u8;
                let dst = (self.virt_addr as usize + offset) as *mut u8;
                let mut b = 0usize;
                while b < copy_bytes {
                    core::ptr::write_volatile(dst.add(b), core::ptr::read(src.add(b)));
                    b += 1;
                }
            }
            row_y += 1;
        }
    }

    fn scroll_up_pixels_shadow(
        self,
        x_left: u32,
        x_right: u32,
        y_top: u32,
        y_bottom: u32,
        dy: u32,
        fill: u32,
    ) {
        if dy == 0
            || y_top >= y_bottom
            || y_bottom > self.height
            || dy >= (y_bottom - y_top)
            || x_left >= x_right
            || x_right > self.width
            || !self.is_present()
        {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let row_copy_bytes = ((x_right - x_left) as usize).saturating_mul(bpp);
        let x_offset_bytes = (x_left as usize).saturating_mul(bpp);
        let copy_rows = (y_bottom - y_top).saturating_sub(dy) as usize;
        let base = self.draw_target_base();

        let mut r = 0usize;
        while r < copy_rows {
            let dst_y = (y_top as usize) + r;
            let src_y = dst_y + (dy as usize);
            let dst_addr = base + dst_y * row_stride + x_offset_bytes;
            let src_addr = base + src_y * row_stride + x_offset_bytes;
            unsafe {
                core::ptr::copy(src_addr as *const u8, dst_addr as *mut u8, row_copy_bytes);
            }
            r += 1;
        }
        fill_buffer_rect(
            base,
            row_stride,
            x_left,
            y_bottom - dy,
            x_right,
            y_bottom,
            bpp,
            fill,
        );
    }
}

#[cfg(target_os = "none")]
struct Console {
    fb: Framebuffer,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    win_left: u32,
    win_right: u32,
    win_top: u32,
    win_bottom: u32,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    cursor_visible: bool,
    cursor_drawn: bool,
    ansi_state: u8,
    params: [u16; 4],
    param_count: usize,
    dirty_y_min: u32,
    dirty_y_max: u32,
    sel_start: Option<(u32, u32)>,
    sel_end: Option<(u32, u32)>,
    cells: [[TermCell; TERM_MAX_COLS]; TERM_MAX_ROWS],
}

#[cfg(target_os = "none")]
const ENABLE_VIRTIO_GPU_IPC_NOTIFY: bool = false;

#[cfg(target_os = "none")]
fn notify_virtio_gpu_flush(x: u32, y: u32, w: u32, h: u32) {
    if !ENABLE_VIRTIO_GPU_IPC_NOTIFY {
        return;
    }
    #[repr(C)]
    struct VirtioReq {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }
    let mut req = VirtioReq {
        sender_pid: 0,
        msg_type: syscall::VIRTIO_GPU_MSG_TRANSFER_FLUSH,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    req.payload[0..4].copy_from_slice(&x.to_le_bytes());
    req.payload[4..8].copy_from_slice(&y.to_le_bytes());
    req.payload[8..12].copy_from_slice(&w.to_le_bytes());
    req.payload[12..16].copy_from_slice(&h.to_le_bytes());

    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            13, // virtio_drivers endpoint
            &req as *const VirtioReq as u64,
            core::mem::size_of::<VirtioReq>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        );
    }
}

#[cfg(not(target_os = "none"))]
fn notify_virtio_gpu_flush(_x: u32, _y: u32, _w: u32, _h: u32) {}

#[cfg(target_os = "none")]
fn notify_virtio_gpu_move_cursor(x: u32, y: u32) {
    if !ENABLE_VIRTIO_GPU_IPC_NOTIFY {
        return;
    }
    #[repr(C)]
    struct VirtioCursorReq {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }
    let mut req = VirtioCursorReq {
        sender_pid: 0,
        msg_type: syscall::VIRTIO_GPU_MSG_MOVE_CURSOR,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    req.payload[0..4].copy_from_slice(&x.to_le_bytes());
    req.payload[4..8].copy_from_slice(&y.to_le_bytes());

    unsafe {
        let _ = syscall::syscall6(
            syscall::SYS_IPC_SEND,
            13, // virtio_drivers endpoint
            &req as *const VirtioCursorReq as u64,
            core::mem::size_of::<VirtioCursorReq>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        );
    }
}

#[cfg(not(target_os = "none"))]
fn notify_virtio_gpu_move_cursor(_x: u32, _y: u32) {}

#[cfg(target_os = "none")]
impl Console {
    const fn new() -> Self {
        Self {
            fb: Framebuffer::absent(),
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            win_left: TERM_LEFT_X,
            win_right: 1002,
            win_top: HEADER_TOTAL_H,
            win_bottom: 724,
            fg: TERM_DEFAULT_FG,
            bg: TERM_DEFAULT_BG,
            cursor_visible: true,
            cursor_drawn: false,
            ansi_state: ANSI_GROUND,
            params: [0; 4],
            param_count: 0,
            dirty_y_min: u32::MAX,
            dirty_y_max: 0,
            sel_start: None,
            sel_end: None,
            cells: [[TermCell::empty(); TERM_MAX_COLS]; TERM_MAX_ROWS],
        }
    }

    #[inline]
    fn dirty_overlaps(&self, y0: u32, y1: u32) -> bool {
        self.dirty_y_min < self.dirty_y_max && self.dirty_y_min < y1 && self.dirty_y_max > y0
    }

    #[inline]
    fn mark_dirty_rows(&mut self, y_start: u32, y_end: u32) {
        if y_start < self.dirty_y_min {
            self.dirty_y_min = y_start;
        }
        if y_end > self.dirty_y_max {
            self.dirty_y_max = y_end.min(self.fb.height);
        }
    }

    #[inline]
    fn flush_damage(&mut self) {
        if self.dirty_y_min < self.dirty_y_max {
            let y_min = self.dirty_y_min;
            let y_max = self.dirty_y_max;
            let h = y_max.saturating_sub(y_min);
            notify_virtio_gpu_flush(0, y_min, self.fb.width, h);
            self.fb.flush_rows(y_min, y_max);
            self.dirty_y_min = u32::MAX;
            self.dirty_y_max = 0;
        }
    }

    fn update_geometry_from_wayland(&mut self) {
        if !self.fb.is_present() {
            return;
        }
        wayland_mut().ensure_terminal_bounds(self.fb);
        let (wx, wy, ww, wh) = wayland_mut().terminal_window_bounds(self.fb);
        let left = wx.saturating_add(10);
        let right = wx.saturating_add(ww).saturating_sub(8);
        let top = wy.saturating_add(WIN_TITLE_H + 6);
        let bottom = wy.saturating_add(wh).saturating_sub(2);
        self.win_left = left;
        self.win_right = right;
        self.win_top = top;
        self.win_bottom = bottom;
        let usable_w = right.saturating_sub(left);
        let usable_h = bottom.saturating_sub(top);
        self.cols = (usable_w / CHAR_W).min(TERM_MAX_COLS as u32).max(1);
        self.rows = (usable_h / CHAR_H).min(TERM_MAX_ROWS as u32).max(1);
        if self.col >= self.cols {
            self.col = self.cols.saturating_sub(1);
        }
        if self.row >= self.rows {
            self.row = self.rows.saturating_sub(1);
        }
    }

    #[inline]
    fn has_active_selection(&self) -> bool {
        matches!((self.sel_start, self.sel_end), (Some(a), Some(b)) if a != b)
    }

    fn normalized_selection(&self) -> Option<((u32, u32), (u32, u32))> {
        let (a, b) = (self.sel_start?, self.sel_end?);
        if a == b {
            return None;
        }
        let idx_a = (a.1 as usize) * TERM_MAX_COLS + (a.0 as usize);
        let idx_b = (b.1 as usize) * TERM_MAX_COLS + (b.0 as usize);
        if idx_a <= idx_b {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }

    #[inline]
    fn is_cell_selected(&self, col: u32, row: u32) -> bool {
        let Some((a, b)) = self.normalized_selection() else {
            return false;
        };
        let idx = (row as usize) * TERM_MAX_COLS + (col as usize);
        let min_i = (a.1 as usize) * TERM_MAX_COLS + (a.0 as usize);
        let max_i = (b.1 as usize) * TERM_MAX_COLS + (b.0 as usize);
        idx >= min_i && idx <= max_i
    }

    fn pixel_to_cell(&self, mx: u32, my: u32) -> Option<(u32, u32)> {
        if self.cols == 0
            || self.rows == 0
            || mx < self.win_left
            || mx >= self.win_right
            || my < self.win_top
            || my >= self.win_bottom
        {
            return None;
        }
        let c = ((mx - self.win_left) / CHAR_W).min(self.cols - 1);
        let r = ((my - self.win_top) / CHAR_H).min(self.rows - 1);
        Some((c, r))
    }

    fn pixel_to_cell_clamped(&self, mx: u32, my: u32) -> (u32, u32) {
        let max_c = self.cols.saturating_sub(1);
        let max_r = self.rows.saturating_sub(1);
        let c = if mx <= self.win_left {
            0
        } else {
            ((mx - self.win_left) / CHAR_W).min(max_c)
        };
        let r = if my <= self.win_top {
            0
        } else {
            ((my - self.win_top) / CHAR_H).min(max_r)
        };
        (c, r)
    }

    fn extract_selection(&self, out: &mut [u8; 4096]) -> usize {
        let Some(((c0, r0), (c1, r1))) = self.normalized_selection() else {
            return 0;
        };
        let mut out_len = 0usize;
        let max_c = self.cols.saturating_sub(1);
        let max_r = self.rows.saturating_sub(1);
        let r_end = r1.min(max_r);
        let mut r = r0.min(max_r);
        while r <= r_end && out_len < out.len() {
            let start_c = if r == r0 { c0.min(max_c) } else { 0 };
            let end_c = if r == r1 { c1.min(max_c) } else { max_c };
            let mut last_non_space = None;
            let mut c = start_c;
            while c <= end_c {
                let ch = self.cells[r as usize][c as usize].ch;
                if ch != b' ' && ch != 0 {
                    last_non_space = Some(c);
                }
                c += 1;
            }
            if let Some(last_c) = last_non_space {
                let mut k = start_c;
                while k <= last_c && out_len < out.len() {
                    let ch = self.cells[r as usize][k as usize].ch;
                    out[out_len] = if ch == 0 { b' ' } else { ch };
                    out_len += 1;
                    k += 1;
                }
            }
            if r < r_end && out_len < out.len() {
                out[out_len] = b'\n';
                out_len += 1;
            }
            r += 1;
        }
        out_len
    }

    fn redraw_cell_rows(&mut self, fb: Framebuffer, r0: u32, r1: u32) {
        if !fb.is_present() || wayland_mut().surfaces[0].minimized || self.rows == 0 || self.cols == 0 {
            return;
        }
        let max_r = self.rows - 1;
        let start_r = r0.min(max_r);
        let end_r = r1.min(max_r);
        if start_r > end_r {
            return;
        }
        let mut r = start_r;
        while r <= end_r {
            let mut c = 0u32;
            while c < self.cols {
                self.draw_cell_at(c, r, self.cells[r as usize][c as usize]);
                c += 1;
            }
            r += 1;
        }
        if self.row >= start_r && self.row <= end_r {
            self.draw_cursor(true);
        }
        let y0 = self.top_offset_px().saturating_add(start_r.saturating_mul(CHAR_H));
        let y1 = self.top_offset_px().saturating_add((end_r + 1).saturating_mul(CHAR_H));
        self.mark_dirty_rows(y0, y1);
    }

    #[inline]
    fn left_offset_px(&self) -> u32 {
        self.win_left
    }

    #[inline]
    fn right_limit_px(&self) -> u32 {
        self.win_right
    }

    #[inline]
    fn top_offset_px(&self) -> u32 {
        self.win_top
    }

    #[inline]
    fn bottom_limit_px(&self) -> u32 {
        self.win_bottom
    }

    fn clear_cells(&mut self) {
        let mut r = 0usize;
        while r < TERM_MAX_ROWS {
            let mut c = 0usize;
            while c < TERM_MAX_COLS {
                self.cells[r][c] = TermCell::empty();
                c += 1;
            }
            r += 1;
        }
    }

    fn configure(&mut self, fb: Framebuffer) {
        self.fb = fb;
        self.sel_start = None;
        self.sel_end = None;
        self.update_geometry_from_wayland();
        self.col = 0;
        self.row = 0;
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
        self.cursor_drawn = false;
        self.clear_cells();
        if self.fb.is_present() {
            desktop::render_desktop_base_with_wayland(self.fb, wayland_mut());
            desktop_mut().render_active_overlay(self.fb);
            if wayland_mut().kickoff_open && self.fb.height > DOCK_BAR_H + 260 && self.fb.width > 380 {
                desktop::render_kickoff_menu(
                    self.fb,
                    wayland_mut().selected_category,
                    wayland_mut().selected_app,
                );
            }
            desktop::render_bottom_panel_with_wayland(self.fb, wayland_mut());
            self.mark_dirty_rows(0, self.fb.height);
            self.flush_damage();
        }
    }

    fn clear(&mut self) {
        self.col = 0;
        self.row = 0;
        self.cursor_drawn = false;
        self.sel_start = None;
        self.sel_end = None;
        self.clear_cells();
        if self.fb.is_present() {
            self.update_geometry_from_wayland();
            if !wayland_mut().surfaces[0].minimized {
                let (wx, wy, ww, wh) = wayland_mut().terminal_window_bounds(self.fb);
                let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
                let y0 = wy.saturating_add(WIN_TITLE_H);
                let h = wh.saturating_sub(WIN_TITLE_H + 2);
                self.fb.fill_rect_shadow(wx + 2, y0, ww.saturating_sub(4), h, bg);
                desktop_mut().render_active_overlay(self.fb);
                if wayland_mut().kickoff_open && self.fb.height > DOCK_BAR_H + 260 && self.fb.width > 380 {
                    desktop::render_kickoff_menu(
                        self.fb,
                        wayland_mut().selected_category,
                        wayland_mut().selected_app,
                    );
                }
                self.mark_dirty_rows(y0, y0.saturating_add(h));
            }
        }
    }

    fn draw_cell_at(&self, col: u32, row: u32, cell: TermCell) {
        if !self.fb.is_present() || col >= self.cols || row >= self.rows {
            return;
        }
        let selected = self.is_cell_selected(col, row);
        let (fg_rgb, bg_rgb) = if selected {
            ([0xff, 0xff, 0xff], [0x29, 0x62, 0xa8])
        } else {
            (cell.fg, cell.bg)
        };
        let fg = self.fb.encode(fg_rgb[0], fg_rgb[1], fg_rgb[2]);
        let bg = self.fb.encode(bg_rgb[0], bg_rgb[1], bg_rgb[2]);
        let x = self.left_offset_px().saturating_add(col.saturating_mul(CHAR_W));
        let y = self.top_offset_px().saturating_add(row.saturating_mul(CHAR_H));

        if cell.ch == b' ' {
            self.fb.fill_rect_shadow(x, y, CHAR_W, CHAR_H, bg);
            return;
        }

        let glyph = glyph_for(cell.ch);
        let bpp = self.fb.bytes_per_pixel() as usize;
        let stride = (self.fb.stride_pixels as usize).saturating_mul(bpp);
        let base = self.fb.draw_target_base();

        let mut gy = 0usize;
        while gy < FONT_GLYPH_HEIGHT {
            let bits = glyph[gy];
            let row_y = y.saturating_add(gy as u32);
            if row_y < self.fb.height {
                let row_addr = base
                    .saturating_add((row_y as usize).saturating_mul(stride))
                    .saturating_add((x as usize).saturating_mul(bpp));
                if bpp == 4 {
                    let mut gx = 0usize;
                    while gx < FONT_GLYPH_WIDTH {
                        let c0 = if (bits & (0x80 >> gx)) != 0 { fg } else { bg };
                        let c1 = if (bits & (0x80 >> (gx + 1))) != 0 { fg } else { bg };
                        let pair = (c0 as u64) | ((c1 as u64) << 32);
                        unsafe {
                            core::ptr::write((row_addr + gx * 4) as *mut u64, pair);
                        }
                        gx += 2;
                    }
                } else {
                    let mut gx = 0usize;
                    while gx < FONT_GLYPH_WIDTH {
                        let color = if (bits & (0x80 >> gx)) != 0 { fg } else { bg };
                        let bytes = color.to_le_bytes();
                        let addr = row_addr + gx * 3;
                        unsafe {
                            core::ptr::write(addr as *mut u8, bytes[0]);
                            core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                            core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                        }
                        gx += 1;
                    }
                }
            }
            gy += 1;
        }
    }

    fn redraw_all(&mut self, fb: Framebuffer) {
        if !fb.is_present() || wayland_mut().surfaces[0].minimized {
            return;
        }
        self.fb = fb;
        self.update_geometry_from_wayland();
        let def_bg = [TERM_DEFAULT_BG.0, TERM_DEFAULT_BG.1, TERM_DEFAULT_BG.2];
        let mut r = 0u32;
        while r < self.rows {
            let mut c = 0u32;
            while c < self.cols {
                let cell = self.cells[r as usize][c as usize];
                if cell.ch != b' ' || cell.bg != def_bg || self.is_cell_selected(c, r) {
                    self.draw_cell_at(c, r, cell);
                }
                c += 1;
            }
            r += 1;
        }
        self.draw_cursor(true);
    }

    fn draw_cursor(&mut self, visible: bool) {
        if !self.fb.is_present() || !self.cursor_visible || wayland_mut().surfaces[0].minimized {
            self.cursor_drawn = false;
            return;
        }
        let x = self.left_offset_px().saturating_add(
            self.col
                .min(self.cols.saturating_sub(1))
                .saturating_mul(CHAR_W),
        );
        let row_y = self.top_offset_px().saturating_add(
            self.row
                .min(self.rows.saturating_sub(1))
                .saturating_mul(CHAR_H),
        );
        let y = row_y.saturating_add(CHAR_H.saturating_sub(2));
        let pixel = if visible {
            self.fb.encode(self.fg.0, self.fg.1, self.fg.2)
        } else {
            self.fb.encode(self.bg.0, self.bg.1, self.bg.2)
        };
        self.fb.fill_rect_shadow(x, y, CHAR_W, 2, pixel);
        self.mark_dirty_rows(y, y + 2);
        self.cursor_drawn = visible;
    }

    fn scroll_lines(&mut self, lines: u32) {
        if !self.fb.is_present() {
            return;
        }
        self.sel_start = None;
        self.sel_end = None;
        if self.cursor_drawn {
            self.draw_cursor(false);
        }
        let shift = (lines.max(1) as usize).min(self.rows as usize);
        let rows_n = self.rows as usize;
        let cols_n = self.cols as usize;
        let mut r = 0usize;
        while r + shift < rows_n {
            let mut c = 0usize;
            while c < cols_n {
                self.cells[r][c] = self.cells[r + shift][c];
                c += 1;
            }
            r += 1;
        }
        while r < rows_n {
            let mut c = 0usize;
            while c < cols_n {
                self.cells[r][c] = TermCell::empty();
                c += 1;
            }
            r += 1;
        }

        if !wayland_mut().surfaces[0].minimized {
            let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
            let dy = lines.max(1).saturating_mul(CHAR_H);
            let (wx, _, ww, _) = wayland_mut().terminal_window_bounds(self.fb);
            let x_left = wx.saturating_add(2);
            let x_right = wx.saturating_add(ww).saturating_sub(2);
            let y0 = self.top_offset_px();
            let y1 = y0.saturating_add(self.rows.saturating_mul(CHAR_H)).min(self.bottom_limit_px());
            self.fb.scroll_up_pixels_shadow(x_left, x_right, y0, y1, dy, bg);
            self.mark_dirty_rows(y0, y1);
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.scroll_lines(1);
        }
    }

    fn draw_ascii(&mut self, byte: u8) {
        if !self.fb.is_present() {
            return;
        }
        let c = self.col.min(self.cols.saturating_sub(1));
        let r = self.row.min(self.rows.saturating_sub(1));
        let cell = TermCell {
            ch: byte,
            fg: [self.fg.0, self.fg.1, self.fg.2],
            bg: [self.bg.0, self.bg.1, self.bg.2],
        };
        self.cells[r as usize][c as usize] = cell;

        if !wayland_mut().surfaces[0].minimized {
            let y = self.top_offset_px().saturating_add(r.saturating_mul(CHAR_H));
            self.mark_dirty_rows(y, y + CHAR_H);
            self.draw_cell_at(c, r, cell);
        }

        self.col += 1;
        if self.col >= self.cols {
            self.newline();
        }
    }

    fn reset_ansi(&mut self) {
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
    }

    fn write_byte(&mut self, byte: u8) {
        match self.ansi_state {
            ANSI_ESC => {
                if byte == b'[' {
                    self.ansi_state = ANSI_CSI;
                    self.params = [0; 4];
                    self.param_count = 0;
                } else {
                    self.reset_ansi();
                }
                return;
            }
            ANSI_CSI => {
                self.write_csi_byte(byte);
                return;
            }
            _ => {}
        }

        match byte {
            0x1b => self.ansi_state = ANSI_ESC,
            0x0c => self.clear(),
            b'\r' => self.col = 0,
            b'\n' => self.newline(),
            0x08 | 0x7f => {
                if self.col > 0 {
                    self.col -= 1;
                }
            }
            b'\t' => {
                let mut n = 4 - (self.col % 4);
                while n != 0 {
                    self.draw_ascii(b' ');
                    n -= 1;
                }
            }
            byte if byte.is_ascii_graphic() || byte == b' ' => self.draw_ascii(byte),
            _ => {}
        }
    }

    fn write_csi_byte(&mut self, byte: u8) {
        if byte.is_ascii_digit() {
            let idx = self.param_count.min(self.params.len() - 1);
            self.params[idx] = self.params[idx]
                .saturating_mul(10)
                .saturating_add((byte - b'0') as u16);
            return;
        }
        if byte == b';' {
            if self.param_count + 1 < self.params.len() {
                self.param_count += 1;
            }
            return;
        }
        match byte {
            b'H' | b'f' => {
                let row = self.params[0].saturating_sub(1) as u32;
                let col = self.params[1].saturating_sub(1) as u32;
                self.row = row.min(self.rows.saturating_sub(1));
                self.col = col.min(self.cols.saturating_sub(1));
            }
            b'J' => {
                if self.params[0] == 2 || self.params[0] == 0 {
                    self.clear();
                }
            }
            b'K' => {
                let r = self.row.min(self.rows.saturating_sub(1)) as usize;
                let mut c = self.col.min(self.cols.saturating_sub(1)) as usize;
                while c < self.cols as usize {
                    self.cells[r][c] = TermCell::empty();
                    c += 1;
                }
                if !wayland_mut().surfaces[0].minimized {
                    let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
                    let y = self.top_offset_px().saturating_add(self.row.saturating_mul(CHAR_H));
                    let x_start = self.left_offset_px().saturating_add(self.col.saturating_mul(CHAR_W));
                    let w = self.right_limit_px().saturating_sub(x_start);
                    self.fb.fill_rect_shadow(
                        x_start,
                        y,
                        w,
                        CHAR_H,
                        bg,
                    );
                    self.mark_dirty_rows(y, y + CHAR_H);
                }
            }
            b'm' => self.apply_sgr(),
            _ => {}
        }
        self.reset_ansi();
    }

    fn apply_sgr(&mut self) {
        let count = self.param_count + 1;
        let mut i = 0usize;
        while i < count {
            match self.params[i] {
                0 => {
                    self.fg = TERM_DEFAULT_FG;
                    self.bg = TERM_DEFAULT_BG;
                }
                7 => {
                    let tmp = self.fg;
                    self.fg = self.bg;
                    self.bg = tmp;
                }
                27 => {
                    self.fg = TERM_DEFAULT_FG;
                    self.bg = TERM_DEFAULT_BG;
                }
                30 => self.fg = (0x00, 0x00, 0x00),
                31 => self.fg = (0xff, 0x6b, 0x6b),
                32 => self.fg = (0x60, 0xff, 0x90),
                33 => self.fg = (0xff, 0xd1, 0x66),
                34 => self.fg = (0x71, 0xa7, 0xff),
                35 => self.fg = (0xd6, 0x7b, 0xff),
                36 => self.fg = (0x62, 0xe6, 0xff),
                37 => self.fg = TERM_DEFAULT_FG,
                _ => {}
            }
            i += 1;
        }
    }

    fn write_all_no_cursor(&mut self, bytes: &[u8]) {
        if !self.fb.is_present() {
            return;
        }
        if self.cursor_drawn {
            self.draw_cursor(false);
        }
        for &byte in bytes {
            self.write_byte(byte);
        }
    }

    fn write_all(&mut self, bytes: &[u8]) {
        self.write_all_no_cursor(bytes);
        self.draw_cursor(true);
        self.flush_damage();
    }
}

#[cfg(target_os = "none")]
struct ConsoleCell(UnsafeCell<Console>);

#[cfg(target_os = "none")]
unsafe impl Sync for ConsoleCell {}

#[cfg(target_os = "none")]
static CONSOLE: ConsoleCell = ConsoleCell(UnsafeCell::new(Console::new()));

#[cfg(target_os = "none")]
fn console_mut() -> &'static mut Console {
    unsafe { &mut *CONSOLE.0.get() }
}

#[cfg(target_os = "none")]
struct DesktopCell(UnsafeCell<DesktopState>);
#[cfg(target_os = "none")]
unsafe impl Sync for DesktopCell {}
#[cfg(target_os = "none")]
static DESKTOP: DesktopCell = DesktopCell(UnsafeCell::new(DesktopState::new()));
#[cfg(target_os = "none")]
fn desktop_mut() -> &'static mut DesktopState {
    unsafe { &mut *DESKTOP.0.get() }
}

#[cfg(target_os = "none")]
struct WaylandCell(UnsafeCell<WaylandState>);
#[cfg(target_os = "none")]
unsafe impl Sync for WaylandCell {}
#[cfg(target_os = "none")]
static WAYLAND: WaylandCell = WaylandCell(UnsafeCell::new(WaylandState::new()));
#[cfg(target_os = "none")]
fn wayland_mut() -> &'static mut WaylandState {
    unsafe { &mut *WAYLAND.0.get() }
}

#[cfg(target_os = "none")]
fn debug_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
        );
    }
}

#[cfg(target_os = "none")]
fn boot_log(bytes: &[u8]) {
    debug_log(bytes);
}

#[cfg(target_os = "none")]
fn exit_failed() -> ! {
    unsafe {
        let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
        let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_os = "none")]
fn register_endpoint() -> bool {
    let name = b"fb_server";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::FB_SERVER_ENDPOINT,
        )
    };
    rc >= 0
}

#[cfg(target_os = "none")]
fn map_framebuffer() -> bool {
    let mut info = syscall::FramebufferInfoWire::default();
    let info_rc = unsafe {
        syscall::syscall1(
            syscall::SYS_FRAMEBUFFER_INFO,
            &mut info as *mut syscall::FramebufferInfoWire as u64,
        )
    };
    if info_rc < 0
        || info.phys_addr == 0
        || info.width == 0
        || info.height == 0
        || info.size_bytes == 0
    {
        boot_log(b"fb_server: framebuffer_info invalid\n");
        return false;
    }
    let virt = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, info.phys_addr, info.size_bytes) };
    if virt <= 0 {
        boot_log(b"fb_server: mmio_map failed\n");
        return false;
    }
    let shadow_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_MMAP,
            0,
            info.size_bytes,
            3,    // PROT_READ | PROT_WRITE
            0x22, // MAP_PRIVATE | MAP_ANONYMOUS
            u64::MAX,
            0,
        )
    };
    let shadow_addr = if shadow_rc > 0 {
        boot_log(b"fb_server: shadow buffer mapped\n");
        shadow_rc as u64
    } else {
        boot_log(b"fb_server: shadow buffer unavailable, using direct mmio\n");
        0
    };
    console_mut().configure(Framebuffer {
        virt_addr: virt as u64,
        shadow_addr,
        width: info.width,
        height: info.height,
        stride_pixels: info.stride_pixels,
        bpp: info.bpp,
        format: info.format,
        size_bytes: info.size_bytes,
    });
    true
}

#[cfg(target_os = "none")]
fn send_reply(endpoint: u64, status: i64, len: u32) {
    if endpoint == 0 {
        return;
    }
    let reply = syscall::FbReply {
        status,
        len,
        _pad: 0,
    };
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &reply as *const syscall::FbReply as u64,
            core::mem::size_of::<syscall::FbReply>() as u64,
            0,
            0,
            0,
        )
    };
}

#[cfg(target_os = "none")]
fn handle_in_batch(req: &syscall::FbRequest) -> syscall::FbReply {
    match req.msg_type {
        syscall::FB_MSG_WRITE => {
            let n = core::cmp::min(req.a as usize, syscall::FB_TEXT_MAX);
            console_mut().write_all_no_cursor(&req.data[..n]);
            syscall::FbReply {
                status: 0,
                len: n as u32,
                _pad: 0,
            }
        }
        syscall::FB_MSG_CLEAR => {
            console_mut().clear();
            syscall::FbReply::default()
        }
        syscall::FB_MSG_SCROLL => {
            console_mut().scroll_lines(req.a as u32);
            syscall::FbReply::default()
        }
        syscall::FB_MSG_SET_CURSOR => {
            let console = console_mut();
            if console.cursor_drawn {
                console.draw_cursor(false);
            }
            console.col = (req.a as u32).min(console.cols.saturating_sub(1));
            console.row = ((req.b & 0xffff_ffff) as u32).min(console.rows.saturating_sub(1));
            console.cursor_visible = req.b & (1u64 << 63) != 0;
            syscall::FbReply::default()
        }
        syscall::DISPLAY_OP_GET_INFO => {
            let fb = console_mut().fb;
            syscall::FbReply {
                status: ((fb.width as i64) << 32) | (fb.height as i64),
                len: fb.bpp,
                _pad: fb.format,
            }
        }
        syscall::DISPLAY_OP_CREATE_SURFACE => {
            let w = (req.a & 0xFFFF_FFFF) as u32;
            let h = (req.b & 0xFFFF_FFFF) as u32;
            if let Some(idx) = wayland_mut().alloc_surface(req.sender_pid, w.max(64), h.max(64)) {
                let s = wayland_mut().surfaces[idx];
                let return_addr = if s.shm_handle != 0 { s.shm_handle } else { s.virt_addr };
                syscall::FbReply {
                    status: return_addr as i64,
                    len: s.surface_id,
                    _pad: s.stride_bytes,
                }
            } else {
                syscall::FbReply {
                    status: syscall::ENOMEM,
                    len: 0,
                    _pad: 0,
                }
            }
        }
        syscall::DISPLAY_OP_DESTROY_SURFACE => {
            let ok = wayland_mut().destroy_surface(req.a as u32, req.sender_pid);
            syscall::FbReply {
                status: if ok { 0 } else { syscall::ENOENT },
                len: 0,
                _pad: 0,
            }
        }
        syscall::DISPLAY_OP_COMMIT_DAMAGE | syscall::DISPLAY_OP_FLIP => {
            let target_surface_id = if (req.a >> 32) != 0 {
                (req.a >> 32) as u32
            } else {
                req.a as u32
            };
            let rect = syscall::DamageRect {
                x: (req.a & 0xFFFF) as u32,
                y: ((req.a >> 16) & 0xFFFF) as u32,
                width: (req.b & 0xFFFF) as u32,
                height: ((req.b >> 16) & 0xFFFF) as u32,
            };
            let fb = console_mut().fb;
            let wire_cnt = wayland_mut().wire_messages_processed;
            let surf_cnt = wayland_mut().active_surface_count();
            desktop_mut().set_wayland_stats(wire_cnt, surf_cnt);
            if let Some((y_min, y_max)) =
                wayland_mut().commit_surface_damage(target_surface_id, req.sender_pid, rect, fb)
            {
                render_band_layers(fb, y_min, y_max);
                console_mut().mark_dirty_rows(y_min.min(fb.height), y_max.min(fb.height));
            } else {
                console_mut().mark_dirty_rows(0, fb.height);
            }
            syscall::FbReply::default()
        }
        syscall::DISPLAY_OP_WAYLAND_WIRE => {
            let raw_len = if req.a != 0 { req.a } else { req.b };
            let n = core::cmp::min(raw_len as usize, syscall::FB_TEXT_MAX);
            let fb = console_mut().fb;
            let (count, dirty) = wayland_mut().dispatch_wire(req.sender_pid, &req.data[..n], fb);
            let wire_cnt = wayland_mut().wire_messages_processed;
            let surf_cnt = wayland_mut().active_surface_count();
            desktop_mut().set_wayland_stats(wire_cnt, surf_cnt);
            if let Some((y_min, y_max)) = dirty {
                if y_min == 0 && y_max >= fb.height {
                    render_all_layers(fb);
                } else {
                    desktop::render_bottom_panel_with_wayland(fb, wayland_mut());
                }
                console_mut().mark_dirty_rows(y_min.min(fb.height), fb.height);
            }
            syscall::FbReply {
                status: count,
                len: n as u32,
                _pad: 0,
            }
        }
        _ => syscall::FbReply {
            status: syscall::EINVAL,
            len: 0,
            _pad: 0,
        },
    }
}

#[cfg(target_os = "none")]
const MOUSE_CURSOR_W: u32 = 12;
#[cfg(target_os = "none")]
const MOUSE_CURSOR_H: u32 = 18;

#[cfg(target_os = "none")]
static MOUSE_CURSOR_BITMAP: [[u8; 12]; 18] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 2, 1, 0, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 3, 2, 1, 0, 0, 0, 0],
    [1, 2, 3, 3, 3, 3, 3, 2, 1, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 0, 0],
    [1, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 0],
    [1, 2, 2, 1, 2, 3, 1, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 1, 2, 3, 1, 0, 0, 0, 0],
    [1, 1, 0, 0, 1, 2, 3, 1, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 1, 2, 2, 1, 0, 0, 0],
    [0, 0, 0, 0, 0, 1, 2, 2, 1, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

#[cfg(target_os = "none")]
struct MouseState {
    x: u32,
    y: u32,
    visible: bool,
    drawn: bool,
    saved_pixels: [u32; 16 * 20],
    saved_x: u32,
    saved_y: u32,
    saved_w: u32,
    saved_h: u32,
    left_button: bool,
    right_button: bool,
    middle_button: bool,
    drag_surface: Option<(usize, i32, i32)>,
    resize_surface: Option<(usize, u32, u32, u32, u32)>,
    selecting_text: bool,
    clipboard: [u8; 4096],
    clipboard_len: usize,
}

#[cfg(target_os = "none")]
impl MouseState {
    const fn new() -> Self {
        Self {
            x: 512,
            y: 384,
            visible: true,
            drawn: false,
            saved_pixels: [0; 16 * 20],
            saved_x: 0,
            saved_y: 0,
            saved_w: 0,
            saved_h: 0,
            left_button: false,
            right_button: false,
            middle_button: false,
            drag_surface: None,
            resize_surface: None,
            selecting_text: false,
            clipboard: [0; 4096],
            clipboard_len: 0,
        }
    }

    fn set_clipboard(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.clipboard.len());
        self.clipboard[..n].copy_from_slice(&bytes[..n]);
        self.clipboard_len = n;
        desktop_mut().set_clipboard_count(n);
    }

    fn restore_background(&mut self, fb: Framebuffer) {
        if !self.drawn || !fb.is_present() {
            return;
        }
        let w = self.saved_w;
        let h = self.saved_h;
        let bpp = fb.bytes_per_pixel() as usize;
        let target_base = fb.draw_target_base();
        let row_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let mut cy = 0u32;
        while cy < h {
            let row_offset = ((self.saved_y + cy) as usize).saturating_mul(row_stride);
            let mut cx = 0u32;
            while cx < w {
                let pixel = self.saved_pixels[(cy * w + cx) as usize];
                let px_offset = row_offset.saturating_add(((self.saved_x + cx) as usize).saturating_mul(bpp));
                let addr = target_base.saturating_add(px_offset);
                unsafe {
                    match bpp {
                        4 => core::ptr::write(addr as *mut u32, pixel),
                        3 => {
                            let bytes = pixel.to_le_bytes();
                            core::ptr::write(addr as *mut u8, bytes[0]);
                            core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                            core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                        }
                        _ => {}
                    }
                }
                cx += 1;
            }
            cy += 1;
        }
        fb.flush_rect(self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.drawn = false;
    }

    fn draw_cursor(&mut self, fb: Framebuffer) {
        if !self.visible || !fb.is_present() {
            return;
        }
        if self.drawn {
            self.restore_background(fb);
        }
        let x = self.x.min(fb.width.saturating_sub(1));
        let y = self.y.min(fb.height.saturating_sub(1));
        let w = MOUSE_CURSOR_W.min(fb.width.saturating_sub(x));
        let h = MOUSE_CURSOR_H.min(fb.height.saturating_sub(y));
        let bpp = fb.bytes_per_pixel() as usize;
        let target_base = fb.draw_target_base();
        let row_stride = (fb.stride_pixels as usize).saturating_mul(bpp);
        let mut cy = 0u32;
        while cy < h {
            let mut cx = 0u32;
            while cx < w {
                self.saved_pixels[(cy * w + cx) as usize] = fb.read_pixel(x + cx, y + cy);
                cx += 1;
            }
            cy += 1;
        }
        self.saved_x = x;
        self.saved_y = y;
        self.saved_w = w;
        self.saved_h = h;
        let black = fb.encode(0, 0, 0);
        let white = fb.encode(255, 255, 255);
        let cyan = fb.encode(0x00, 0xc5, 0xff);
        cy = 0;
        while cy < h {
            let row_offset = ((y + cy) as usize).saturating_mul(row_stride);
            let mut cx = 0u32;
            while cx < w {
                let pixel = match MOUSE_CURSOR_BITMAP[cy as usize][cx as usize] {
                    1 => Some(black),
                    2 => Some(white),
                    3 => Some(cyan),
                    _ => None,
                };
                if let Some(p) = pixel {
                    let px_offset = row_offset.saturating_add(((x + cx) as usize).saturating_mul(bpp));
                    let addr = target_base.saturating_add(px_offset);
                    unsafe {
                        match bpp {
                            4 => core::ptr::write(addr as *mut u32, p),
                            3 => {
                                let bytes = p.to_le_bytes();
                                core::ptr::write(addr as *mut u8, bytes[0]);
                                core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                                core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                            }
                            _ => {}
                        }
                    }
                }
                cx += 1;
            }
            cy += 1;
        }
        fb.flush_rect(x, y, w, h);
        notify_virtio_gpu_move_cursor(x, y);
        self.drawn = true;
    }

    fn update_position(&mut self, dx: i16, dy: i16, fb: Framebuffer) {
        let max_x = fb.width.saturating_sub(1) as i32;
        let max_y = fb.height.saturating_sub(1) as i32;
        let nx = (self.x as i32 + dx as i32).clamp(0, max_x) as u32;
        let ny = (self.y as i32 + dy as i32).clamp(0, max_y) as u32;
        if nx == self.x && ny == self.y {
            return;
        }
        self.restore_background(fb);
        self.x = nx;
        self.y = ny;
        self.draw_cursor(fb);
    }
}

#[cfg(target_os = "none")]
fn register_input_stream() -> Option<u64> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    let endpoint = ((pid as u64) << 32) | 1;
    let name = b"fb_server_input";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    let req = syscall::InputRequest {
        sender_pid: pid as u32,
        msg_type: syscall::INPUT_MSG_ATTACH,
        reply_endpoint: endpoint,
        event: syscall::InputEventWire::default(),
    };
    let send_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if send_rc < 0 {
        return None;
    }
    Some(endpoint)
}

#[cfg(target_os = "none")]
fn render_all_layers(fb: Framebuffer) {
    if !fb.is_present() {
        return;
    }
    desktop::render_desktop_base_with_wayland(fb, wayland_mut());
    console_mut().redraw_all(fb);
    desktop_mut().render_active_overlay(fb);
    if wayland_mut().kickoff_open && fb.height > DOCK_BAR_H + 260 && fb.width > 380 {
        desktop::render_kickoff_menu(
            fb,
            wayland_mut().selected_category,
            wayland_mut().selected_app,
        );
    }
    desktop::render_bottom_panel_with_wayland(fb, wayland_mut());
}

#[cfg(target_os = "none")]
fn render_band_layers(fb: Framebuffer, y0: u32, y1: u32) {
    if !fb.is_present() {
        return;
    }
    let desktop_h = fb.height.saturating_sub(DOCK_BAR_H);
    let by0 = y0.min(desktop_h);
    let by1 = y1.min(desktop_h);
    if by0 < by1 {
        desktop::render_wallpaper_rect(fb, 0, by0, fb.width, by1 - by0);
    }
    wayland_mut().composite_all_surfaces(fb);
    console_mut().redraw_all(fb);
    desktop_mut().render_active_overlay(fb);
    if wayland_mut().kickoff_open && fb.height > DOCK_BAR_H + 260 && fb.width > 380 {
        desktop::render_kickoff_menu(
            fb,
            wayland_mut().selected_category,
            wayland_mut().selected_app,
        );
    }
    if y1 > desktop_h {
        desktop::render_bottom_panel_with_wayland(fb, wayland_mut());
    }
}

#[cfg(target_os = "none")]
fn push_keyboard_bytes(bytes: &[u8]) {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32;
    for &b in bytes {
        let ev = syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_KEYBOARD,
            state: syscall::INPUT_KEY_PRESSED,
            code: 0,
            value: 1,
            ascii: b,
            modifiers: 0,
            _pad: [0; 4],
        };
        let req = syscall::InputRequest {
            sender_pid: pid,
            msg_type: syscall::INPUT_MSG_PUSH,
            reply_endpoint: 0,
            event: ev,
        };
        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                syscall::INPUT_SERVER_ENDPOINT,
                &req as *const syscall::InputRequest as u64,
                core::mem::size_of::<syscall::InputRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 4,
                0,
                0,
            )
        };
    }
}

#[cfg(target_os = "none")]
fn compositor_repaint(mouse: &mut MouseState, fb: Framebuffer, y0: u32, y1: u32, full: bool) {
    if !fb.is_present() {
        return;
    }
    if mouse.drawn {
        mouse.restore_background(fb);
    }
    if full {
        render_all_layers(fb);
    } else {
        render_band_layers(fb, y0, y1);
    }
    console_mut().mark_dirty_rows(y0.min(fb.height), y1.min(fb.height));
    console_mut().flush_damage();
    mouse.draw_cursor(fb);
}

#[cfg(target_os = "none")]
fn execute_kickoff_action(action: KickoffAction, mouse: &mut MouseState, fb: Framebuffer) {
    match action {
        KickoffAction::OpenTerminal => {
            wayland_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::RunCmd(cmd) => {
            wayland_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(cmd);
        }
        KickoffAction::ToggleOverlay(ov) => {
            desktop_mut().toggle_overlay(ov);
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::ClearTerminal => {
            wayland_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"clear\n");
        }
        KickoffAction::CloseMenu => {
            compositor_repaint(mouse, fb, 0, fb.height, true);
        }
        KickoffAction::Reboot => {
            wayland_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"reboot\n");
        }
        KickoffAction::Shutdown => {
            wayland_mut().focus_surface(0);
            compositor_repaint(mouse, fb, 0, fb.height, true);
            push_keyboard_bytes(b"shutdown\n");
        }
    }
}

#[cfg(target_os = "none")]
fn drain_mouse_events(endpoint: u64, mouse: &mut MouseState, fb: Framebuffer) {
    let mut reply = syscall::InputReply::default();
    let mut drained = 0usize;
    let mut total_dx: i16 = 0;
    let mut total_dy: i16 = 0;
    while drained < 64 {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_EXO_IPC_RECV_NB,
                endpoint,
                &mut reply as *mut syscall::InputReply as u64,
                core::mem::size_of::<syscall::InputReply>() as u64,
                0,
            )
        };
        if rc < core::mem::size_of::<syscall::InputReply>() as i64 {
            break;
        }
        if reply.status == 0 && reply.event.device == syscall::INPUT_DEVICE_MOUSE {
            match reply.event.code {
                0x0110 => total_dx = total_dx.saturating_add(reply.event.value),
                0x0111 => total_dy = total_dy.saturating_add(reply.event.value),
                0 => {
                    total_dx = total_dx.saturating_add(reply.event.value);
                    total_dy = total_dy.saturating_add((reply.event.ascii as i8) as i16);
                }
                0x0100 | 1 => {
                    let pressed = reply.event.value != 0;
                    let was_pressed = mouse.left_button;
                    mouse.left_button = pressed;
                    if pressed && !was_pressed {
                        if total_dx != 0 || total_dy != 0 {
                            mouse.update_position(total_dx, total_dy, fb);
                            total_dx = 0;
                            total_dy = 0;
                        }
                        let (hud_hit, hud_copy) =
                            desktop_mut().handle_mouse_click_with_copy(mouse.x, mouse.y, fb);
                        if hud_hit {
                            if let Some(txt) = hud_copy {
                                mouse.set_clipboard(txt);
                            }
                            compositor_repaint(mouse, fb, 0, fb.height, true);
                        } else {
                            match wayland_mut().handle_pointer_press(mouse.x, mouse.y, fb) {
                                UiAction::Redraw(y0, y1) => {
                                    compositor_repaint(mouse, fb, y0, y1, true);
                                }
                                UiAction::Kickoff(act) => {
                                    execute_kickoff_action(act, mouse, fb);
                                }
                                UiAction::StartDrag { surf_idx, off_x, off_y } => {
                                    mouse.drag_surface = Some((surf_idx, off_x, off_y));
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::StartResize {
                                    surf_idx,
                                    start_mx,
                                    start_my,
                                    orig_w,
                                    orig_h,
                                } => {
                                    mouse.resize_surface =
                                        Some((surf_idx, start_mx, start_my, orig_w, orig_h));
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::StartTermSelect { .. } => {
                                    // Clear any prior highlight
                                    let old_sel = console_mut().normalized_selection();
                                    console_mut().sel_start = None;
                                    console_mut().sel_end = None;
                                    if let Some((a, b)) = old_sel {
                                        if mouse.drawn {
                                            mouse.restore_background(fb);
                                        }
                                        console_mut().redraw_cell_rows(fb, a.1, b.1);
                                        console_mut().flush_damage();
                                        mouse.draw_cursor(fb);
                                    }
                                    if let Some(cell) = console_mut().pixel_to_cell(mouse.x, mouse.y) {
                                        mouse.selecting_text = true;
                                        console_mut().sel_start = Some(cell);
                                        console_mut().sel_end = Some(cell);
                                    }
                                }
                                UiAction::ClientPointer { .. } => {
                                    compositor_repaint(mouse, fb, 0, fb.height, true);
                                }
                                UiAction::None => {}
                            }
                        }
                    } else if !pressed && was_pressed {
                        if total_dx != 0 || total_dy != 0 {
                            mouse.update_position(total_dx, total_dy, fb);
                            total_dx = 0;
                            total_dy = 0;
                        }
                        if mouse.drag_surface.is_some() || mouse.resize_surface.is_some() {
                            mouse.drag_surface = None;
                            mouse.resize_surface = None;
                        }
                        if mouse.selecting_text {
                            mouse.selecting_text = false;
                            let prev_sel = console_mut().sel_end;
                            let cur = console_mut().pixel_to_cell_clamped(mouse.x, mouse.y);
                            if Some(cur) != prev_sel {
                                let anchor = console_mut().sel_start.unwrap_or(cur);
                                let old_c = prev_sel.unwrap_or(cur);
                                let r0 = anchor.1.min(old_c.1).min(cur.1);
                                let r1 = anchor.1.max(old_c.1).max(cur.1);
                                console_mut().sel_end = Some(cur);
                                if mouse.drawn {
                                    mouse.restore_background(fb);
                                }
                                console_mut().redraw_cell_rows(fb, r0, r1);
                                console_mut().flush_damage();
                                mouse.draw_cursor(fb);
                            }
                            if console_mut().has_active_selection() {
                                let mut clip_tmp = [0u8; 4096];
                                let n = console_mut().extract_selection(&mut clip_tmp);
                                mouse.set_clipboard(&clip_tmp[..n]);
                            }
                        }
                    }
                }
                0x0101 | 0x0102 => {
                    let pressed = reply.event.value != 0;
                    let is_right = reply.event.code == 0x0101;
                    let was = if is_right {
                        mouse.right_button
                    } else {
                        mouse.middle_button
                    };
                    if is_right {
                        mouse.right_button = pressed;
                    } else {
                        mouse.middle_button = pressed;
                    }
                    if pressed && !was && mouse.clipboard_len > 0 {
                        if console_mut().pixel_to_cell(mouse.x, mouse.y).is_some()
                            && !wayland_mut().surfaces[0].minimized
                        {
                            wayland_mut().focus_surface(0);
                            console_mut().sel_start = None;
                            console_mut().sel_end = None;
                            compositor_repaint(mouse, fb, 0, fb.height, true);
                            push_keyboard_bytes(&mouse.clipboard[..mouse.clipboard_len]);
                        }
                    }
                }
                _ => {}
            }
        }
        drained += 1;
    }

    if total_dx != 0 || total_dy != 0 {
        mouse.update_position(total_dx, total_dy, fb);

        if let Some((s_idx, off_x, off_y)) = mouse.drag_surface {
            if mouse.left_button {
                let old_bounds = wayland_mut().surface_window_bounds(s_idx, fb);
                let target_x = mouse.x as i32 - off_x;
                let target_y = mouse.y as i32 - off_y;
                wayland_mut().move_surface(s_idx, target_x, target_y, fb);
                let new_bounds = wayland_mut().surface_window_bounds(s_idx, fb);
                let (mut y0, mut y1) = match (old_bounds, new_bounds) {
                    (Some((_, oy, _, oh)), Some((_, ny, _, nh))) => {
                        (oy.min(ny), oy.saturating_add(oh + 8).max(ny.saturating_add(nh + 8)))
                    }
                    _ => (0, fb.height),
                };
                y1 = y1.min(fb.height);
                if y0 >= y1 {
                    y0 = 0;
                    y1 = fb.height;
                }
                compositor_repaint(mouse, fb, y0, y1, false);
            } else {
                mouse.drag_surface = None;
            }
        } else if let Some((s_idx, start_mx, start_my, orig_w, orig_h)) = mouse.resize_surface {
            if mouse.left_button {
                let old_bounds = wayland_mut().surface_window_bounds(s_idx, fb);
                let dw = mouse.x as i32 - start_mx as i32;
                let dh = mouse.y as i32 - start_my as i32;
                let new_w = (orig_w as i32 + dw).max(320) as u32;
                let new_h = (orig_h as i32 + dh).max(180) as u32;
                wayland_mut().resize_surface(s_idx, new_w, new_h, fb);
                let new_bounds = wayland_mut().surface_window_bounds(s_idx, fb);
                let (y0, y1) = match (old_bounds, new_bounds) {
                    (Some((_, oy, _, oh)), Some((_, ny, _, nh))) => (
                        oy.min(ny),
                        oy.saturating_add(oh + 8)
                            .max(ny.saturating_add(nh + 8))
                            .min(fb.height),
                    ),
                    _ => (0, fb.height),
                };
                compositor_repaint(mouse, fb, y0, y1, false);
            } else {
                mouse.resize_surface = None;
            }
        } else if mouse.selecting_text && mouse.left_button {
            let cur = console_mut().pixel_to_cell_clamped(mouse.x, mouse.y);
            if Some(cur) != console_mut().sel_end {
                let anchor = console_mut().sel_start.unwrap_or(cur);
                let prev = console_mut().sel_end.unwrap_or(cur);
                let r0 = anchor.1.min(prev.1).min(cur.1);
                let r1 = anchor.1.max(prev.1).max(cur.1);
                console_mut().sel_end = Some(cur);
                if mouse.drawn {
                    mouse.restore_background(fb);
                }
                console_mut().redraw_cell_rows(fb, r0, r1);
                console_mut().flush_damage();
                mouse.draw_cursor(fb);
            }
        }
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"fb_server: boot\n");
    let _ = map_framebuffer();
    let mut mouse = MouseState::new();
    if console_mut().fb.is_present() {
        mouse.x = console_mut().fb.width / 2;
        mouse.y = console_mut().fb.height / 2;
        mouse.draw_cursor(console_mut().fb);
    }
    if !register_endpoint() {
        boot_log(b"fb_server: register failed\n");
        exit_failed();
    }
    boot_log(b"fb_server: registered\n");

    let mut input_endpoint = register_input_stream();
    let mut last_clock_min = desktop::current_rtc_minute();
    let wl_sock = wayland::wayland_socket_mut();

    let mut req = syscall::FbRequest::zeroed();
    loop {
        if input_endpoint.is_none() {
            input_endpoint = register_input_stream();
        }
        if let Some(ep) = input_endpoint {
            drain_mouse_events(ep, &mut mouse, console_mut().fb);
        }
        let wl_res = wl_sock.poll(wayland_mut(), console_mut().fb);
        if wl_res.full_repaint || wl_res.dirty.is_some() {
            let fb = console_mut().fb;
            if fb.is_present() {
                desktop_mut().set_wayland_stats(
                    wayland_mut().wire_messages_processed,
                    wayland_mut().active_surface_count(),
                );
                if wl_res.full_repaint {
                    compositor_repaint(&mut mouse, fb, 0, fb.height, true);
                } else if let Some((y0, y1)) = wl_res.dirty {
                    if mouse.drawn {
                        mouse.restore_background(fb);
                    }
                    if y0 == 0 && y1 >= fb.height {
                        render_all_layers(fb);
                    } else {
                        render_band_layers(fb, y0, y1);
                        desktop::render_bottom_panel_with_wayland(fb, wayland_mut());
                    }
                    console_mut().mark_dirty_rows(y0.min(fb.height), fb.height);
                    console_mut().flush_damage();
                    mouse.draw_cursor(fb);
                }
            }
        }
        let now_min = desktop::current_rtc_minute();
        if now_min != last_clock_min && console_mut().fb.is_present() {
            last_clock_min = now_min;
            let fb = console_mut().fb;
            let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
            if mouse.drawn {
                mouse.restore_background(fb);
            }
            desktop::render_bottom_panel_with_wayland(fb, wayland_mut());
            if desktop_mut().active_overlay == DockOverlay::Calendar {
                desktop_mut().render_active_overlay(fb);
                console_mut().mark_dirty_rows(0, fb.height);
            } else {
                console_mut().mark_dirty_rows(dock_y, fb.height);
            }
            console_mut().flush_damage();
            mouse.draw_cursor(fb);
        }
        let rc = if wl_res.did_io {
            unsafe {
                syscall::syscall4(
                    syscall::SYS_EXO_IPC_RECV_NB,
                    syscall::FB_SERVER_ENDPOINT,
                    &mut req as *mut syscall::FbRequest as u64,
                    core::mem::size_of::<syscall::FbRequest>() as u64,
                    0,
                )
            }
        } else {
            unsafe {
                syscall::syscall4(
                    syscall::SYS_IPC_RECV,
                    syscall::FB_SERVER_ENDPOINT,
                    &mut req as *mut syscall::FbRequest as u64,
                    core::mem::size_of::<syscall::FbRequest>() as u64,
                    syscall::IPC_FLAG_TIMEOUT | RECV_TIMEOUT_MS,
                )
            }
        };
        if rc < 0 {
            continue;
        }

        // 1. Restore mouse background before any window/console changes
        if mouse.drawn {
            mouse.restore_background(console_mut().fb);
        }

        // 2. Traiter le premier message + drainer jusqu'à 64 messages en attente en RAM !
        let reply = handle_in_batch(&req);
        send_reply(req.reply_endpoint, reply.status, reply.len);

        let mut batched = 0usize;
        while batched < BATCH_DRAIN_MAX {
            let nb_rc = unsafe {
                syscall::syscall4(
                    syscall::SYS_EXO_IPC_RECV_NB,
                    syscall::FB_SERVER_ENDPOINT,
                    &mut req as *mut syscall::FbRequest as u64,
                    core::mem::size_of::<syscall::FbRequest>() as u64,
                    0,
                )
            };
            if nb_rc < 0 {
                break;
            }
            let r = handle_in_batch(&req);
            send_reply(req.reply_endpoint, r.status, r.len);
            batched += 1;
        }

        // 3. Dessiner le curseur texte et flusher la zone modifiée (DamageRect) vers MMIO en une seule fois
        console_mut().draw_cursor(true);
        console_mut().flush_damage();

        // 4. Redessiner le curseur souris au-dessus uniquement s'il a été retiré
        if !mouse.drawn {
            mouse.draw_cursor(console_mut().fb);
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    boot_log(b"fb_server: PANIC!\n");
    loop {
        core::hint::spin_loop();
    }
}
