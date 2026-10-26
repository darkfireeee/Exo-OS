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
use desktop::{
    DesktopState, DOCK_BAR_H, HEADER_TOTAL_H, TERM_LEFT_X, TERM_RIGHT_MARGIN, WIN_BOTTOM_MARGIN,
};
#[cfg(target_os = "none")]
use wayland::WaylandState;

#[cfg(target_os = "none")]
const CHAR_W: u32 = FONT_GLYPH_WIDTH as u32;
#[cfg(target_os = "none")]
const CHAR_H: u32 = FONT_GLYPH_HEIGHT as u32;
// Doit rester PAIR car bit 0 = IPC_FLAG_TIMEOUT (0x0001).
#[cfg(target_os = "none")]
const RECV_TIMEOUT_MS: u64 = 4;
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

    fn scroll_up_pixels_shadow(self, y_top: u32, y_bottom: u32, dy: u32, fill: u32) {
        if dy == 0 || y_top >= y_bottom || y_bottom > self.height || dy >= (y_bottom - y_top) || !self.is_present() {
            return;
        }
        let bpp = self.bytes_per_pixel() as usize;
        let row_stride = (self.stride_pixels as usize).saturating_mul(bpp);
        let top_bytes = (y_top as usize).saturating_mul(row_stride);
        let dy_bytes = (dy as usize).saturating_mul(row_stride);
        let copy_rows = (y_bottom - y_top).saturating_sub(dy) as usize;
        let copy_bytes = copy_rows.saturating_mul(row_stride);
        let base = self.draw_target_base();

        unsafe {
            core::ptr::copy(
                (base + top_bytes + dy_bytes) as *const u8,
                (base + top_bytes) as *mut u8,
                copy_bytes,
            );
        }
        fill_buffer_rect(
            base,
            row_stride,
            0,
            y_bottom - dy,
            self.width,
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
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    cursor_visible: bool,
    cursor_drawn: bool,
    ansi_state: u8,
    params: [u16; 4],
    param_count: usize,
    dirty_y_min: u32,
    dirty_y_max: u32,
}

#[cfg(target_os = "none")]
impl Console {
    const fn new() -> Self {
        Self {
            fb: Framebuffer::absent(),
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            fg: (0xe6, 0xf1, 0xff),
            bg: (0x03, 0x0d, 0x14),
            cursor_visible: true,
            cursor_drawn: false,
            ansi_state: ANSI_GROUND,
            params: [0; 4],
            param_count: 0,
            dirty_y_min: u32::MAX,
            dirty_y_max: 0,
        }
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
            self.fb.flush_rows(self.dirty_y_min, self.dirty_y_max);
            self.dirty_y_min = u32::MAX;
            self.dirty_y_max = 0;
        }
    }

    #[inline]
    fn left_offset_px(&self) -> u32 {
        if self.fb.width > TERM_LEFT_X + TERM_RIGHT_MARGIN + CHAR_W * 20 {
            TERM_LEFT_X
        } else {
            0
        }
    }

    #[inline]
    fn right_limit_px(&self) -> u32 {
        if self.fb.width > TERM_LEFT_X + TERM_RIGHT_MARGIN + CHAR_W * 20 {
            self.fb.width.saturating_sub(TERM_RIGHT_MARGIN)
        } else {
            self.fb.width
        }
    }

    #[inline]
    fn top_offset_px(&self) -> u32 {
        if self.fb.height > HEADER_TOTAL_H + WIN_BOTTOM_MARGIN + CHAR_H * 4 {
            HEADER_TOTAL_H
        } else {
            0
        }
    }

    #[inline]
    fn bottom_limit_px(&self) -> u32 {
        if self.fb.height > HEADER_TOTAL_H + WIN_BOTTOM_MARGIN + CHAR_H * 4 {
            self.fb.height.saturating_sub(WIN_BOTTOM_MARGIN + 2)
        } else {
            self.fb.height
        }
    }

    fn configure(&mut self, fb: Framebuffer) {
        self.fb = fb;
        let usable_w = self.right_limit_px().saturating_sub(self.left_offset_px());
        self.cols = (usable_w / CHAR_W).max(1);
        let usable_h = self.bottom_limit_px().saturating_sub(self.top_offset_px());
        self.rows = (usable_h / CHAR_H).max(1);
        self.col = 0;
        self.row = 0;
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
        self.clear();
        self.flush_damage();
    }

    fn clear(&mut self) {
        self.col = 0;
        self.row = 0;
        self.cursor_drawn = false;
        if self.fb.is_present() {
            let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
            let y0 = desktop::WIN_TOP_Y + desktop::WIN_TITLE_H;
            let y1 = self.fb.height.saturating_sub(WIN_BOTTOM_MARGIN);
            let x0 = desktop::WIN_LEFT_X;
            let x1 = self.fb.width.saturating_sub(desktop::WIN_RIGHT_MARGIN);
            self.fb.fill_rect_shadow(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0), bg);
            desktop_mut().render_chrome(self.fb);
            self.mark_dirty_rows(0, self.fb.height);
        }
    }

    fn draw_cursor(&mut self, visible: bool) {
        if !self.fb.is_present() || !self.cursor_visible {
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
        if self.cursor_drawn {
            self.draw_cursor(false);
        }
        let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
        let dy = lines.max(1).saturating_mul(CHAR_H);
        let y0 = self.top_offset_px();
        let y1 = y0.saturating_add(self.rows.saturating_mul(CHAR_H)).min(self.bottom_limit_px());
        self.fb.scroll_up_pixels_shadow(y0, y1, dy, bg);
        self.mark_dirty_rows(y0, y1);
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
        let fg = self.fb.encode(self.fg.0, self.fg.1, self.fg.2);
        let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
        let x = self.left_offset_px().saturating_add(self.col.saturating_mul(CHAR_W));
        let y = self.top_offset_px().saturating_add(self.row.saturating_mul(CHAR_H));
        self.mark_dirty_rows(y, y + CHAR_H);

        if byte == b' ' {
            self.fb.fill_rect_shadow(x, y, CHAR_W, CHAR_H, bg);
            self.col += 1;
            if self.col >= self.cols {
                self.newline();
            }
            return;
        }

        let glyph = glyph_for(byte);
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
                    self.fg = (0xe6, 0xf1, 0xff);
                    self.bg = (0x03, 0x0d, 0x14);
                }
                7 => {
                    let tmp = self.fg;
                    self.fg = self.bg;
                    self.bg = tmp;
                }
                27 => {
                    self.fg = (0xe6, 0xf1, 0xff);
                    self.bg = (0x03, 0x0d, 0x14);
                }
                30 => self.fg = (0x00, 0x00, 0x00),
                31 => self.fg = (0xff, 0x6b, 0x6b),
                32 => self.fg = (0x60, 0xff, 0x90),
                33 => self.fg = (0xff, 0xd1, 0x66),
                34 => self.fg = (0x71, 0xa7, 0xff),
                35 => self.fg = (0xd6, 0x7b, 0xff),
                36 => self.fg = (0x62, 0xe6, 0xff),
                37 => self.fg = (0xe6, 0xf1, 0xff),
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
                syscall::FbReply {
                    status: s.virt_addr as i64,
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
            let ok = wayland_mut().destroy_surface(req.a as u32);
            syscall::FbReply {
                status: if ok { 0 } else { syscall::ENOENT },
                len: 0,
                _pad: 0,
            }
        }
        syscall::DISPLAY_OP_COMMIT_DAMAGE | syscall::DISPLAY_OP_FLIP => {
            let rect = syscall::DamageRect {
                x: (req.a & 0xFFFF) as u32,
                y: ((req.a >> 16) & 0xFFFF) as u32,
                width: (req.b & 0xFFFF) as u32,
                height: ((req.b >> 16) & 0xFFFF) as u32,
            };
            let _ = wayland_mut().commit_surface_damage(req.a as u32, rect);
            console_mut().mark_dirty_rows(0, console_mut().fb.height);
            syscall::FbReply::default()
        }
        syscall::DISPLAY_OP_WAYLAND_WIRE => {
            let n = core::cmp::min(req.a as usize, syscall::FB_TEXT_MAX);
            let count = wayland_mut().dispatch_wire(req.sender_pid, &req.data[..n]);
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
    sel_start: Option<(u32, u32)>,
    sel_end: Option<(u32, u32)>,
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
            sel_start: None,
            sel_end: None,
        }
    }

    fn restore_background(&mut self, fb: Framebuffer) {
        if !self.drawn || !fb.is_present() {
            return;
        }
        let w = self.saved_w;
        let h = self.saved_h;
        let mut cy = 0u32;
        while cy < h {
            let mut cx = 0u32;
            while cx < w {
                let pixel = self.saved_pixels[(cy * w + cx) as usize];
                fb.put_pixel_direct(self.saved_x + cx, self.saved_y + cy, pixel);
                cx += 1;
            }
            cy += 1;
        }
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
            let mut cx = 0u32;
            while cx < w {
                match MOUSE_CURSOR_BITMAP[cy as usize][cx as usize] {
                    1 => fb.put_pixel_direct(x + cx, y + cy, black),
                    2 => fb.put_pixel_direct(x + cx, y + cy, white),
                    3 => fb.put_pixel_direct(x + cx, y + cy, cyan),
                    _ => {}
                }
                cx += 1;
            }
            cy += 1;
        }
        self.drawn = true;
    }

    fn update_position(&mut self, dx: i16, dy: i16, fb: Framebuffer) {
        self.restore_background(fb);
        let max_x = fb.width.saturating_sub(1) as i32;
        let max_y = fb.height.saturating_sub(1) as i32;
        let nx = (self.x as i32 + dx as i32).clamp(0, max_x) as u32;
        let ny = (self.y as i32 + dy as i32).clamp(0, max_y) as u32;
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
                        if desktop_mut().handle_mouse_click(mouse.x, mouse.y, fb) {
                            mouse.restore_background(fb);
                            console_mut().clear();
                            console_mut().draw_cursor(true);
                            console_mut().flush_damage();
                            mouse.draw_cursor(fb);
                        }
                    }
                    let cell_col = (mouse.x / CHAR_W).min(console_mut().cols.saturating_sub(1));
                    let cell_row = (mouse.y.saturating_sub(console_mut().top_offset_px()) / CHAR_H)
                        .min(console_mut().rows.saturating_sub(1));
                    if pressed {
                        if mouse.sel_start.is_none() {
                            mouse.sel_start = Some((cell_col, cell_row));
                        }
                        mouse.sel_end = Some((cell_col, cell_row));
                    }
                }
                _ => {}
            }
        }
        drained += 1;
    }
    if total_dx != 0 || total_dy != 0 {
        mouse.update_position(total_dx, total_dy, fb);
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"fb_server: boot\n");
    if !register_endpoint() {
        boot_log(b"fb_server: register failed\n");
        exit_failed();
    }
    boot_log(b"fb_server: registered\n");
    let _ = map_framebuffer();

    let mut input_endpoint = register_input_stream();
    let mut mouse = MouseState::new();
    if console_mut().fb.is_present() {
        mouse.draw_cursor(console_mut().fb);
    }

    let mut req = syscall::FbRequest::zeroed();
    loop {
        if input_endpoint.is_none() {
            input_endpoint = register_input_stream();
        }
        if let Some(ep) = input_endpoint {
            drain_mouse_events(ep, &mut mouse, console_mut().fb);
        }
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::FB_SERVER_ENDPOINT,
                &mut req as *mut syscall::FbRequest as u64,
                core::mem::size_of::<syscall::FbRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | RECV_TIMEOUT_MS,
            )
        };
        if rc < 0 {
            continue;
        }

        // 1. Retirer le curseur souris une seule fois au début du lot
        mouse.restore_background(console_mut().fb);

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
        mouse.draw_cursor(console_mut().fb);
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
