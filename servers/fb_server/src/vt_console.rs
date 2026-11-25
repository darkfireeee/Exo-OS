//! Strate 1 emergency/boot ANSI VT100 terminal console renderer for `fb_server`.

use core::cell::UnsafeCell;

use crate::desktop::{self, DOCK_BAR_H, HEADER_TOTAL_H, TERM_LEFT_X, WIN_TITLE_H};
use crate::desktop_mut;
use crate::framebuffer::{notify_virtio_gpu_flush, Framebuffer};
use crate::shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};
use crate::surface_table::surface_table_mut;

pub const CHAR_W: u32 = FONT_GLYPH_WIDTH as u32;
pub const CHAR_H: u32 = FONT_GLYPH_HEIGHT as u32;
pub const TERM_DEFAULT_FG: (u8, u8, u8) = (0xe6, 0xf1, 0xff);
pub const TERM_DEFAULT_BG: (u8, u8, u8) = (0x12, 0x14, 0x1d);
pub const TERM_MAX_COLS: usize = 256;
pub const TERM_MAX_ROWS: usize = 72;

const ANSI_GROUND: u8 = 0;
const ANSI_ESC: u8 = 1;
const ANSI_CSI: u8 = 2;

#[derive(Clone, Copy)]
pub struct TermCell {
    pub ch: u8,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

impl TermCell {
    pub const fn empty() -> Self {
        Self {
            ch: b' ',
            fg: [TERM_DEFAULT_FG.0, TERM_DEFAULT_FG.1, TERM_DEFAULT_FG.2],
            bg: [TERM_DEFAULT_BG.0, TERM_DEFAULT_BG.1, TERM_DEFAULT_BG.2],
        }
    }
}

pub struct Console {
    pub fb: Framebuffer,
    pub cols: u32,
    pub rows: u32,
    pub col: u32,
    pub row: u32,
    pub win_left: u32,
    pub win_right: u32,
    pub win_top: u32,
    pub win_bottom: u32,
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
    pub cursor_visible: bool,
    pub cursor_drawn: bool,
    ansi_state: u8,
    params: [u16; 4],
    param_count: usize,
    dirty_y_min: u32,
    dirty_y_max: u32,
    pub sel_start: Option<(u32, u32)>,
    pub sel_end: Option<(u32, u32)>,
    cells: [[TermCell; TERM_MAX_COLS]; TERM_MAX_ROWS],
}

impl Console {
    pub const fn new() -> Self {
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
    pub fn mark_dirty_rows(&mut self, y_start: u32, y_end: u32) {
        if y_start < self.dirty_y_min {
            self.dirty_y_min = y_start;
        }
        if y_end > self.dirty_y_max {
            self.dirty_y_max = y_end.min(self.fb.height);
        }
    }

    #[inline]
    pub fn flush_damage(&mut self) {
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

    pub fn update_geometry_from_surfaces(&mut self) {
        if !self.fb.is_present() {
            return;
        }
        surface_table_mut().ensure_terminal_bounds(self.fb);
        let (wx, wy, ww, wh) = surface_table_mut().terminal_window_bounds(self.fb);
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
    pub fn has_active_selection(&self) -> bool {
        matches!((self.sel_start, self.sel_end), (Some(a), Some(b)) if a != b)
    }

    pub fn normalized_selection(&self) -> Option<((u32, u32), (u32, u32))> {
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

    pub fn pixel_to_cell(&self, mx: u32, my: u32) -> Option<(u32, u32)> {
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

    pub fn pixel_to_cell_clamped(&self, mx: u32, my: u32) -> (u32, u32) {
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

    pub fn extract_selection(&self, out: &mut [u8; 4096]) -> usize {
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

    pub fn redraw_cell_rows(&mut self, fb: Framebuffer, r0: u32, r1: u32) {
        if !fb.is_present()
            || surface_table_mut().surfaces[0].minimized
            || self.rows == 0
            || self.cols == 0
        {
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
        let y0 = self.win_top.saturating_add(start_r.saturating_mul(CHAR_H));
        let y1 = self.win_top.saturating_add((end_r + 1).saturating_mul(CHAR_H));
        self.mark_dirty_rows(y0, y1);
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

    pub fn configure(&mut self, fb: Framebuffer) {
        self.fb = fb;
        self.sel_start = None;
        self.sel_end = None;
        self.update_geometry_from_surfaces();
        self.col = 0;
        self.row = 0;
        self.ansi_state = ANSI_GROUND;
        self.params = [0; 4];
        self.param_count = 0;
        self.cursor_drawn = false;
        self.clear_cells();
        if self.fb.is_present() {
            desktop::render_desktop_base_with_wayland(self.fb, surface_table_mut());
            desktop_mut().render_active_overlay(self.fb);
            if surface_table_mut().kickoff_open
                && self.fb.height > DOCK_BAR_H + 260
                && self.fb.width > 380
            {
                desktop::render_kickoff_menu(
                    self.fb,
                    surface_table_mut().selected_category,
                    surface_table_mut().selected_app,
                );
            }
            desktop::render_bottom_panel_with_wayland(self.fb, surface_table_mut());
            self.mark_dirty_rows(0, self.fb.height);
            self.flush_damage();
        }
    }

    pub fn clear(&mut self) {
        self.col = 0;
        self.row = 0;
        self.cursor_drawn = false;
        self.sel_start = None;
        self.sel_end = None;
        self.clear_cells();
        if self.fb.is_present() {
            self.update_geometry_from_surfaces();
            if !surface_table_mut().surfaces[0].minimized {
                let (wx, wy, ww, wh) = surface_table_mut().terminal_window_bounds(self.fb);
                let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
                let y0 = wy.saturating_add(WIN_TITLE_H);
                let h = wh.saturating_sub(WIN_TITLE_H + 2);
                self.fb.fill_rect_shadow(wx + 2, y0, ww.saturating_sub(4), h, bg);
                desktop_mut().render_active_overlay(self.fb);
                if surface_table_mut().kickoff_open
                    && self.fb.height > DOCK_BAR_H + 260
                    && self.fb.width > 380
                {
                    desktop::render_kickoff_menu(
                        self.fb,
                        surface_table_mut().selected_category,
                        surface_table_mut().selected_app,
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
        let x = self.win_left.saturating_add(col.saturating_mul(CHAR_W));
        let y = self.win_top.saturating_add(row.saturating_mul(CHAR_H));

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

    pub fn redraw_all(&mut self, fb: Framebuffer) {
        if !fb.is_present() || surface_table_mut().surfaces[0].minimized {
            return;
        }
        self.fb = fb;
        self.update_geometry_from_surfaces();
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

    pub fn draw_cursor(&mut self, visible: bool) {
        if !self.fb.is_present()
            || !self.cursor_visible
            || surface_table_mut().surfaces[0].minimized
        {
            self.cursor_drawn = false;
            return;
        }
        let x = self.win_left.saturating_add(
            self.col
                .min(self.cols.saturating_sub(1))
                .saturating_mul(CHAR_W),
        );
        let row_y = self.win_top.saturating_add(
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

    pub fn scroll_lines(&mut self, lines: u32) {
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

        if !surface_table_mut().surfaces[0].minimized {
            let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
            let dy = lines.max(1).saturating_mul(CHAR_H);
            let (wx, _, ww, _) = surface_table_mut().terminal_window_bounds(self.fb);
            let x_left = wx.saturating_add(2);
            let x_right = wx.saturating_add(ww).saturating_sub(2);
            let y0 = self.win_top;
            let y1 = y0.saturating_add(self.rows.saturating_mul(CHAR_H)).min(self.win_bottom);
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

        if !surface_table_mut().surfaces[0].minimized {
            let y = self.win_top.saturating_add(r.saturating_mul(CHAR_H));
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
                if !surface_table_mut().surfaces[0].minimized {
                    let bg = self.fb.encode(self.bg.0, self.bg.1, self.bg.2);
                    let y = self.win_top.saturating_add(self.row.saturating_mul(CHAR_H));
                    let x_start = self.win_left.saturating_add(self.col.saturating_mul(CHAR_W));
                    let w = self.win_right.saturating_sub(x_start);
                    self.fb.fill_rect_shadow(x_start, y, w, CHAR_H, bg);
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

    pub fn write_all_no_cursor(&mut self, bytes: &[u8]) {
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
}

struct ConsoleCell(UnsafeCell<Console>);
unsafe impl Sync for ConsoleCell {}

static CONSOLE: ConsoleCell = ConsoleCell(UnsafeCell::new(Console::new()));

pub fn console_mut() -> &'static mut Console {
    unsafe { &mut *CONSOLE.0.get() }
}
