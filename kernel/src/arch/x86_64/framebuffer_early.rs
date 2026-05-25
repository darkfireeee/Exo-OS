//! # framebuffer_early.rs — Console framebuffer minimale de boot
//!
//! Chemin de boot graphique utilisé après `arch_boot_init()`, quand :
//! - la carte framebuffer a été récupérée depuis Multiboot2 ou exo-boot ;
//! - l'espace d'adressage kernel est prêt à mapper une fenêtre MMIO dédiée.
//!
//! Contraintes :
//! - zéro allocation ;
//! - fallback VGA inchangé si aucun framebuffer n'est disponible ;
//! - rendu simple, lisible et stable sous QEMU `-vga std`.

use spin::Mutex;

use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
use crate::memory::core::{align_up, Frame, PageFlags, PhysAddr, VirtAddr, FIXMAP_BASE, PAGE_SIZE};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;
use core::sync::atomic::{compiler_fence, Ordering};

use super::boot::early_init::{BootFramebufferFormat, BootInfo};

#[path = "../../../../exo-boot/src/display/font.rs"]
mod shared_font;

use shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};

const FRAMEBUFFER_WINDOW_OFFSET: u64 = 0x0200_0000;
const FRAMEBUFFER_VIRT_BASE: u64 = FIXMAP_BASE.as_u64() + FRAMEBUFFER_WINDOW_OFFSET;
const TITLE_SCALE: u32 = 3;
const SUBTITLE_SCALE: u32 = 2;
const BODY_SCALE: u32 = 1;
const STAGE_START_Y: u32 = 252;
const STAGE_ROW_HEIGHT: u32 = 23;
const STAGE_PANEL_WIDTH: u32 = 720;
const TERM_MARGIN_X: u32 = 24;
const TERM_HEADER_H: u32 = 54;
const TERM_MARGIN_BOTTOM: u32 = 18;
const TERM_SCALE: u32 = 1;
const ANSI_GROUND: u8 = 0;
const ANSI_ESC: u8 = 1;
const ANSI_CSI: u8 = 2;

#[derive(Debug, Clone, Copy)]
struct Framebuffer {
    virt_addr: u64,
    width: u32,
    height: u32,
    stride: u32,
    bpp: u32,
    format: BootFramebufferFormat,
    size_bytes: u64,
}

impl Framebuffer {
    const fn absent() -> Self {
        Self {
            virt_addr: 0,
            width: 0,
            height: 0,
            stride: 0,
            bpp: 0,
            format: BootFramebufferFormat::None,
            size_bytes: 0,
        }
    }

    fn is_present(&self) -> bool {
        self.virt_addr != 0
            && self.width != 0
            && self.height != 0
            && self.size_bytes != 0
            && self.bpp >= 24
            && self.stride >= self.width
    }

    fn bytes_per_pixel(&self) -> usize {
        ((self.bpp as usize).saturating_add(7) / 8).max(1)
    }

    fn encode_rgb(&self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            BootFramebufferFormat::Rgbx => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
            BootFramebufferFormat::Bgrx => ((b as u32) << 16) | ((g as u32) << 8) | (r as u32),
            BootFramebufferFormat::Unknown | BootFramebufferFormat::None => {
                ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
            }
        }
    }

    fn put_pixel(&self, x: u32, y: u32, pixel: u32) {
        if !self.is_present() || x >= self.width || y >= self.height {
            return;
        }

        let bpp = self.bytes_per_pixel();
        let offset = (y as usize)
            .saturating_mul(self.stride as usize)
            .saturating_add(x as usize)
            .saturating_mul(bpp);
        let addr = (self.virt_addr as usize).saturating_add(offset);

        // SAFETY: le framebuffer est mappé page par page par `map_framebuffer_window()`;
        // `x`/`y` sont bornés à `width`/`height`, donc l'offset reste dans la fenêtre mappée.
        unsafe {
            match bpp {
                4 => core::ptr::write_volatile(addr as *mut u32, pixel),
                3 => {
                    let bytes = pixel.to_le_bytes();
                    core::ptr::write_volatile(addr as *mut u8, bytes[0]);
                    core::ptr::write_volatile((addr + 1) as *mut u8, bytes[1]);
                    core::ptr::write_volatile((addr + 2) as *mut u8, bytes[2]);
                }
                _ => {}
            }
        }
    }

    fn read_pixel(&self, x: u32, y: u32) -> u32 {
        if !self.is_present() || x >= self.width || y >= self.height {
            return 0;
        }

        let bpp = self.bytes_per_pixel();
        let offset = (y as usize)
            .saturating_mul(self.stride as usize)
            .saturating_add(x as usize)
            .saturating_mul(bpp);
        let addr = (self.virt_addr as usize).saturating_add(offset);

        // SAFETY: same bounds and mapping invariant as `put_pixel`.
        unsafe {
            match bpp {
                4 => core::ptr::read_volatile(addr as *const u32),
                3 => {
                    let b0 = core::ptr::read_volatile(addr as *const u8);
                    let b1 = core::ptr::read_volatile((addr + 1) as *const u8);
                    let b2 = core::ptr::read_volatile((addr + 2) as *const u8);
                    u32::from_le_bytes([b0, b1, b2, 0])
                }
                _ => 0,
            }
        }
    }

    fn fill_rect(&self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
        if !self.is_present() || w == 0 || h == 0 {
            return;
        }

        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        for row in y..y_end {
            for col in x..x_end {
                self.put_pixel(col, row, pixel);
            }
        }
    }

    fn scroll_rect_up(&self, x: u32, y: u32, w: u32, h: u32, dy: u32, fill: u32) {
        if !self.is_present() || w == 0 || h == 0 || dy == 0 || dy >= h {
            return;
        }

        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        let copy_start = y.saturating_add(dy);
        let mut row = copy_start;
        while row < y_end {
            let mut col = x;
            while col < x_end {
                let pixel = self.read_pixel(col, row);
                self.put_pixel(col, row - dy, pixel);
                col = col.saturating_add(1);
            }
            row = row.saturating_add(1);
        }
        self.fill_rect(
            x,
            y_end.saturating_sub(dy),
            x_end.saturating_sub(x),
            dy,
            fill,
        );
    }

    fn draw_line(&self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, pixel: u32) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            if x0 >= 0 && y0 >= 0 {
                self.put_pixel(x0 as u32, y0 as u32, pixel);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = err * 2;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn draw_glyph_scaled(&self, x: u32, y: u32, ch: u8, scale: u32, fg: u32, bg: u32) {
        let glyph = glyph_for(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..FONT_GLYPH_WIDTH as u32 {
                let bit = (bits >> (FONT_GLYPH_WIDTH as u32 - 1 - col)) & 1;
                let pixel = if bit != 0 { fg } else { bg };
                let px = x + col * scale;
                let py = y + row as u32 * scale;
                self.fill_rect(px, py, scale, scale, pixel);
            }
        }
    }

    fn draw_text_scaled(&self, x: u32, y: u32, text: &str, scale: u32, fg: u32, bg: u32) {
        let advance = FONT_GLYPH_WIDTH as u32 * scale;
        let line_height = FONT_GLYPH_HEIGHT as u32 * scale;
        let mut cursor_x = x;
        let mut cursor_y = y;

        for ch in text.bytes() {
            match ch {
                b'\n' => {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(line_height);
                }
                b'\r' => {
                    cursor_x = x;
                }
                _ => {
                    self.draw_glyph_scaled(cursor_x, cursor_y, ch, scale, fg, bg);
                    cursor_x = cursor_x.saturating_add(advance);
                }
            }
        }
    }

    fn measure_text_width(text: &str, scale: u32) -> u32 {
        (text.len() as u32)
            .saturating_mul(FONT_GLYPH_WIDTH as u32)
            .saturating_mul(scale)
    }

    fn draw_text_centered(&self, y: u32, text: &str, scale: u32, fg: u32, bg: u32) {
        let width = Self::measure_text_width(text, scale);
        let x = self.width.saturating_sub(width) / 2;
        self.draw_text_scaled(x, y, text, scale, fg, bg);
    }
}

struct ConsoleState {
    fb: Framebuffer,
    stage_count: u32,
    terminal_mode: bool,
    term_col: u32,
    term_row: u32,
    term_fg_code: u8,
    term_bold: bool,
    term_reverse: bool,
    ansi_state: u8,
    ansi_code: u16,
}

impl ConsoleState {
    const fn new() -> Self {
        Self {
            fb: Framebuffer::absent(),
            stage_count: 0,
            terminal_mode: false,
            term_col: 0,
            term_row: 0,
            term_fg_code: 39,
            term_bold: false,
            term_reverse: false,
            ansi_state: ANSI_GROUND,
            ansi_code: 0,
        }
    }
}

static CONSOLE: Mutex<ConsoleState> = Mutex::new(ConsoleState::new());

#[inline]
fn bg_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(6, 18, 28)
}

#[inline]
fn panel_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(12, 31, 45)
}

#[inline]
fn accent_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(0, 197, 255)
}

#[inline]
fn accent_dark(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(0, 115, 155)
}

#[inline]
fn ok_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(56, 215, 124)
}

#[inline]
fn text_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(233, 247, 255)
}

#[inline]
fn muted_text_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(143, 177, 194)
}

#[inline]
fn ansi_color(fb: &Framebuffer, code: u8, bold: bool) -> u32 {
    match (code, bold) {
        (30, _) => fb.encode_rgb(85, 95, 104),
        (31, false) => fb.encode_rgb(226, 86, 86),
        (31, true) | (91, _) => fb.encode_rgb(255, 112, 112),
        (32, false) => fb.encode_rgb(70, 196, 112),
        (32, true) | (92, _) => fb.encode_rgb(105, 224, 145),
        (33, false) => fb.encode_rgb(220, 190, 85),
        (33, true) | (93, _) => fb.encode_rgb(255, 221, 105),
        (34, false) => fb.encode_rgb(82, 160, 255),
        (34, true) | (94, _) => fb.encode_rgb(116, 188, 255),
        (35, false) => fb.encode_rgb(197, 120, 220),
        (35, true) | (95, _) => fb.encode_rgb(224, 150, 248),
        (36, false) => fb.encode_rgb(45, 196, 215),
        (36, true) | (96, _) => fb.encode_rgb(90, 221, 235),
        (37, false) => fb.encode_rgb(214, 225, 232),
        (37, true) | (97, _) => fb.encode_rgb(248, 252, 255),
        (90, _) => fb.encode_rgb(125, 139, 148),
        _ => text_color(fb),
    }
}

#[inline]
fn terminal_fg_color(fb: &Framebuffer, state: &ConsoleState) -> u32 {
    ansi_color(fb, state.term_fg_code, state.term_bold)
}

#[inline]
fn term_bg_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(2, 10, 16)
}

#[inline]
fn term_header_color(fb: &Framebuffer) -> u32 {
    fb.encode_rgb(8, 27, 39)
}

#[inline]
fn term_cols(fb: &Framebuffer) -> u32 {
    let char_w = FONT_GLYPH_WIDTH as u32 * TERM_SCALE;
    fb.width
        .saturating_sub(TERM_MARGIN_X.saturating_mul(2))
        .saturating_div(char_w.max(1))
        .max(1)
}

#[inline]
fn term_rows(fb: &Framebuffer) -> u32 {
    let char_h = FONT_GLYPH_HEIGHT as u32 * TERM_SCALE;
    fb.height
        .saturating_sub(TERM_HEADER_H)
        .saturating_sub(TERM_MARGIN_BOTTOM)
        .saturating_div(char_h.max(1))
        .max(1)
}

#[inline]
fn term_origin_y() -> u32 {
    TERM_HEADER_H
}

fn render_terminal_shell(state: &mut ConsoleState) {
    let fb = state.fb;
    if !fb.is_present() {
        return;
    }

    let bg = term_bg_color(&fb);
    let header = term_header_color(&fb);
    let accent = accent_color(&fb);
    let fg = text_color(&fb);
    let muted = muted_text_color(&fb);

    fb.fill_rect(0, 0, fb.width, fb.height, bg);
    fb.fill_rect(0, 0, fb.width, TERM_HEADER_H, header);
    fb.fill_rect(0, TERM_HEADER_H.saturating_sub(2), fb.width, 2, accent);
    fb.draw_text_scaled(TERM_MARGIN_X, 14, "EXO-OS", BODY_SCALE, accent, header);
    fb.draw_text_scaled(
        TERM_MARGIN_X + 88,
        14,
        "userspace console",
        BODY_SCALE,
        fg,
        header,
    );
    fb.draw_text_scaled(
        TERM_MARGIN_X,
        34,
        "init_server services ready - exosh interactive session",
        BODY_SCALE,
        muted,
        header,
    );

    state.terminal_mode = true;
    state.term_col = 0;
    state.term_row = 0;
}

fn terminal_clear_locked(state: &mut ConsoleState) {
    let fb = state.fb;
    if !fb.is_present() {
        return;
    }
    reset_terminal_attrs(state);
    if !state.terminal_mode {
        render_terminal_shell(state);
        return;
    }
    fb.fill_rect(
        0,
        TERM_HEADER_H,
        fb.width,
        fb.height.saturating_sub(TERM_HEADER_H),
        term_bg_color(&fb),
    );
    state.term_col = 0;
    state.term_row = 0;
}

fn reset_terminal_attrs(state: &mut ConsoleState) {
    state.term_fg_code = 39;
    state.term_bold = false;
    state.term_reverse = false;
    state.ansi_state = ANSI_GROUND;
    state.ansi_code = 0;
}

fn apply_sgr_code(state: &mut ConsoleState, code: u16) {
    match code {
        0 => reset_terminal_attrs(state),
        1 => state.term_bold = true,
        7 => state.term_reverse = true,
        22 => state.term_bold = false,
        27 => state.term_reverse = false,
        30..=37 | 90..=97 => state.term_fg_code = code as u8,
        39 => state.term_fg_code = 39,
        _ => {}
    }
}

fn terminal_consume_ansi_locked(state: &mut ConsoleState, byte: u8) -> bool {
    match state.ansi_state {
        ANSI_GROUND => {
            if byte == 0x1b {
                state.ansi_state = ANSI_ESC;
                state.ansi_code = 0;
                true
            } else {
                false
            }
        }
        ANSI_ESC => {
            if byte == b'[' {
                state.ansi_state = ANSI_CSI;
                state.ansi_code = 0;
            } else {
                state.ansi_state = ANSI_GROUND;
            }
            true
        }
        ANSI_CSI => {
            match byte {
                b'0'..=b'9' => {
                    state.ansi_code = state
                        .ansi_code
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as u16);
                }
                b';' => {
                    apply_sgr_code(state, state.ansi_code);
                    state.ansi_code = 0;
                }
                b'm' => {
                    apply_sgr_code(state, state.ansi_code);
                    state.ansi_code = 0;
                    state.ansi_state = ANSI_GROUND;
                }
                _ => {
                    state.ansi_code = 0;
                    state.ansi_state = ANSI_GROUND;
                }
            }
            true
        }
        _ => {
            state.ansi_state = ANSI_GROUND;
            false
        }
    }
}

fn terminal_newline_locked(state: &mut ConsoleState) {
    let fb = state.fb;
    let rows = term_rows(&fb);
    state.term_col = 0;
    state.term_row = state.term_row.saturating_add(1);
    if state.term_row >= rows {
        let char_h = FONT_GLYPH_HEIGHT as u32 * TERM_SCALE;
        let y = term_origin_y();
        let h = fb
            .height
            .saturating_sub(y)
            .saturating_sub(TERM_MARGIN_BOTTOM);
        fb.scroll_rect_up(0, y, fb.width, h, char_h, term_bg_color(&fb));
        state.term_row = rows.saturating_sub(1);
    }
}

fn terminal_backspace_locked(state: &mut ConsoleState) {
    let fb = state.fb;
    if state.term_col == 0 {
        return;
    }

    state.term_col = state.term_col.saturating_sub(1);
    let char_w = FONT_GLYPH_WIDTH as u32 * TERM_SCALE;
    let char_h = FONT_GLYPH_HEIGHT as u32 * TERM_SCALE;
    let x = TERM_MARGIN_X.saturating_add(state.term_col.saturating_mul(char_w));
    let y = term_origin_y().saturating_add(state.term_row.saturating_mul(char_h));
    fb.fill_rect(x, y, char_w, char_h, term_bg_color(&fb));
}

fn terminal_put_printable_locked(state: &mut ConsoleState, byte: u8) {
    let fb = state.fb;
    let cols = term_cols(&fb);
    if state.term_col >= cols {
        terminal_newline_locked(state);
    }

    let char_w = FONT_GLYPH_WIDTH as u32 * TERM_SCALE;
    let char_h = FONT_GLYPH_HEIGHT as u32 * TERM_SCALE;
    let x = TERM_MARGIN_X.saturating_add(state.term_col.saturating_mul(char_w));
    let y = term_origin_y().saturating_add(state.term_row.saturating_mul(char_h));
    let normal_bg = term_bg_color(&fb);
    let normal_fg = terminal_fg_color(&fb, state);
    let (fg, bg) = if state.term_reverse {
        (normal_bg, normal_fg)
    } else {
        (normal_fg, normal_bg)
    };
    fb.draw_glyph_scaled(x, y, byte, TERM_SCALE, fg, bg);
    state.term_col = state.term_col.saturating_add(1);
    if state.term_col >= cols {
        terminal_newline_locked(state);
    }
}

fn terminal_write_byte_locked(state: &mut ConsoleState, byte: u8) {
    if !state.terminal_mode {
        render_terminal_shell(state);
    }
    if terminal_consume_ansi_locked(state, byte) {
        return;
    }

    match byte {
        0x0c => terminal_clear_locked(state),
        0x08 | 0x7f => terminal_backspace_locked(state),
        b'\n' => terminal_newline_locked(state),
        b'\r' => state.term_col = 0,
        b'\t' => {
            let mut n = 0;
            while n < 4 {
                terminal_put_printable_locked(state, b' ');
                n += 1;
            }
        }
        0x20..=0x7e => terminal_put_printable_locked(state, byte),
        _ => {}
    }
}

fn map_framebuffer_window(info: &BootInfo) -> Option<u64> {
    if info.framebuffer_phys_addr == 0 || info.framebuffer_size_bytes == 0 {
        return None;
    }

    let phys_base = info.framebuffer_phys_addr & !(PAGE_SIZE as u64 - 1);
    let page_off = info.framebuffer_phys_addr - phys_base;
    let map_size = align_up((page_off + info.framebuffer_size_bytes) as usize, PAGE_SIZE) as u64;
    let pages = (map_size as usize) / PAGE_SIZE;
    let flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::WRITE_COMBINING
        | PageFlags::GLOBAL;

    for idx in 0..pages {
        let virt = VirtAddr::new(FRAMEBUFFER_VIRT_BASE + (idx * PAGE_SIZE) as u64);
        let phys = PhysAddr::new(phys_base + (idx * PAGE_SIZE) as u64);
        if let Some(mapped) = KERNEL_AS.translate(virt) {
            if mapped.as_u64() != phys.as_u64() {
                return None;
            }
            continue;
        }

        // SAFETY: `virt` vise une fenêtre kernel dédiée au framebuffer de boot,
        // `phys` pointe vers une page framebuffer/MMIO reportée par le bootloader,
        // et `KERNEL_AS` est déjà initialisé par `arch_boot_init()`.
        unsafe {
            KERNEL_AS
                .map(virt, Frame::containing(phys), flags, &KERNEL_FAULT_ALLOC)
                .ok()?;
        }
    }

    compiler_fence(Ordering::SeqCst);
    // SAFETY: boot-time local flush on the BSP before the first framebuffer MMIO
    // write; this makes release/SMP see newly-created upper-level page entries.
    unsafe {
        tlb::flush_all_including_global();
    }
    compiler_fence(Ordering::SeqCst);

    Some(FRAMEBUFFER_VIRT_BASE + page_off)
}

fn draw_logo(fb: &Framebuffer) {
    let fg = accent_color(fb);
    let fg_dark = accent_dark(fb);
    let bg = bg_color(fb);
    let center_x = fb.width / 2;
    let top_y = 44u32;
    let icon_size = 42i32;
    let cx = center_x as i32 - 170;
    let cy = top_y as i32 + 36;

    fb.draw_line(cx, cy - icon_size, cx + icon_size, cy, fg);
    fb.draw_line(cx + icon_size, cy, cx, cy + icon_size, fg);
    fb.draw_line(cx, cy + icon_size, cx - icon_size, cy, fg);
    fb.draw_line(cx - icon_size, cy, cx, cy - icon_size, fg);
    fb.draw_line(cx - 30, cy - 30, cx + 30, cy + 30, fg_dark);
    fb.draw_line(cx + 30, cy - 30, cx - 30, cy + 30, fg_dark);

    fb.draw_text_scaled(center_x - 92, top_y, "EXO-OS", TITLE_SCALE, fg, bg);
    fb.draw_text_centered(
        top_y + 72,
        "FRAMEBUFFER BOOT PATH ACTIVE",
        SUBTITLE_SCALE,
        text_color(fb),
        bg,
    );
    fb.draw_text_centered(
        top_y + 104,
        "Kernel progress follows the real init phases",
        BODY_SCALE,
        muted_text_color(fb),
        bg,
    );
}

fn draw_stage_line(fb: &Framebuffer, row: u32, label: &str, status: &str, ok: bool) {
    let y = STAGE_START_Y + row * STAGE_ROW_HEIGHT;
    let left = fb.width.saturating_sub(760) / 2;
    let panel = panel_color(fb);
    let fg = text_color(fb);
    let muted = muted_text_color(fb);
    let chip = if ok { ok_color(fb) } else { accent_dark(fb) };
    let chip_text = fb.encode_rgb(6, 18, 28);

    fb.fill_rect(left, y, STAGE_PANEL_WIDTH, 18, panel);
    fb.fill_rect(left, y + 18, STAGE_PANEL_WIDTH, 1, accent_dark(fb));
    fb.draw_text_scaled(left + 14, y + 3, label, BODY_SCALE, fg, panel);
    fb.draw_text_scaled(
        left + 430,
        y + 3,
        "..............",
        BODY_SCALE,
        muted,
        panel,
    );
    fb.fill_rect(left + 590, y + 2, 104, 15, chip);
    fb.draw_text_scaled(left + 634, y + 3, status, BODY_SCALE, chip_text, chip);
}

fn render_screen(state: &ConsoleState) {
    let fb = &state.fb;
    if !fb.is_present() {
        return;
    }

    fb.fill_rect(0, 0, fb.width, fb.height, bg_color(fb));
    fb.fill_rect(0, 0, fb.width, 18, accent_color(fb));
    fb.fill_rect(0, 18, fb.width, 6, accent_dark(fb));
    draw_logo(fb);

    fb.draw_text_centered(
        STAGE_START_Y - 28,
        "INITIALISATION DES MODULES",
        BODY_SCALE,
        muted_text_color(fb),
        bg_color(fb),
    );

    let rows = state.stage_count;
    for idx in 0..rows {
        // Les libellés sont redessinés lors des mises à jour directes ; au redraw initial,
        // on affiche juste le panneau d'arrière-plan.
        let y = STAGE_START_Y + idx * STAGE_ROW_HEIGHT;
        let left = fb.width.saturating_sub(STAGE_PANEL_WIDTH) / 2;
        fb.fill_rect(left, y, STAGE_PANEL_WIDTH, 18, panel_color(fb));
        fb.fill_rect(left, y + 18, STAGE_PANEL_WIDTH, 1, accent_dark(fb));
    }
}

/// Active le framebuffer de boot si le bootloader en a fourni un.
pub fn init_from_boot_info(info: &BootInfo) -> bool {
    if info.framebuffer_phys_addr == 0
        || info.framebuffer_size_bytes == 0
        || info.framebuffer_width == 0
        || info.framebuffer_height == 0
        || info.framebuffer_bpp < 24
    {
        return false;
    }

    let virt_addr = match map_framebuffer_window(info) {
        Some(addr) => addr,
        None => return false,
    };

    let mut state = CONSOLE.lock();
    state.fb = Framebuffer {
        virt_addr,
        width: info.framebuffer_width,
        height: info.framebuffer_height,
        stride: info.framebuffer_stride,
        bpp: info.framebuffer_bpp,
        format: info.framebuffer_format,
        size_bytes: info.framebuffer_size_bytes,
    };
    state.stage_count = 0;
    state.terminal_mode = false;
    state.term_col = 0;
    state.term_row = 0;
    reset_terminal_attrs(&mut state);
    render_screen(&state);
    true
}

/// `true` si la console framebuffer est active.
pub fn is_active() -> bool {
    CONSOLE.lock().fb.is_present()
}

/// Affiche un message de progression dans la liste des modules.
pub fn stage_ok(label: &str) {
    let mut state = CONSOLE.lock();
    let fb = state.fb;
    if !fb.is_present() {
        return;
    }

    draw_stage_line(&fb, state.stage_count, label, "OK", true);
    state.stage_count = state.stage_count.saturating_add(1);
}

/// Affiche l'écran final de boot.
pub fn boot_complete() {
    let state = CONSOLE.lock();
    let fb = state.fb;
    if !fb.is_present() {
        return;
    }

    let panel = panel_color(&fb);
    let ok = ok_color(&fb);
    let y = fb.height.saturating_sub(44);
    let x = fb.width.saturating_sub(820) / 2;

    fb.fill_rect(x, y, 820, 28, panel);
    fb.fill_rect(x, y, 820, 2, accent_color(&fb));
    fb.draw_text_centered(y + 8, "EXO-OS KERNEL BOOT COMPLETE", BODY_SCALE, ok, panel);
}

/// Affiche le statut du handoff userspace sans masquer les modules deja OK.
pub fn userspace_status(title: &str, detail: &str, hint: &str) {
    let state = CONSOLE.lock();
    let fb = state.fb;
    if !fb.is_present() {
        return;
    }

    let panel = panel_color(&fb);
    let fg = text_color(&fb);
    let warn = fb.encode_rgb(255, 209, 102);
    let muted = muted_text_color(&fb);
    let stage_bottom = STAGE_START_Y
        .saturating_add(state.stage_count.max(1).saturating_mul(STAGE_ROW_HEIGHT))
        .saturating_add(24);
    let y = stage_bottom.min(fb.height.saturating_sub(170));
    let x = fb.width.saturating_sub(860) / 2;

    fb.fill_rect(x, y, 860, 82, panel);
    fb.fill_rect(x, y, 860, 2, warn);
    fb.draw_text_centered(y + 10, title, SUBTITLE_SCALE, warn, panel);
    fb.draw_text_centered(y + 44, detail, BODY_SCALE, fg, panel);
    fb.draw_text_centered(y + 62, hint, BODY_SCALE, muted, panel);

    let terminal_y = y.saturating_add(96);
    let footer_guard = 54u32;
    if terminal_y.saturating_add(70) < fb.height.saturating_sub(footer_guard) {
        let term_bg = fb.encode_rgb(3, 13, 20);
        let term_border = accent_dark(&fb);
        fb.fill_rect(x, terminal_y, 860, 72, term_bg);
        fb.fill_rect(x, terminal_y, 860, 2, term_border);
        fb.draw_text_scaled(
            x + 14,
            terminal_y + 12,
            "TTY/SHELL SURFACE",
            BODY_SCALE,
            accent_color(&fb),
            term_bg,
        );
        fb.draw_text_scaled(
            x + 14,
            terminal_y + 36,
            "exosh: waiting for tty_server readiness and /dev/tty attach",
            BODY_SCALE,
            muted,
            term_bg,
        );
    }
}

/// Efface et prepare la surface de terminal framebuffer.
pub fn terminal_clear() {
    let mut state = CONSOLE.lock();
    if !state.fb.is_present() {
        return;
    }
    terminal_clear_locked(&mut state);
}

/// Ecrit des octets sur la console interactive framebuffer avec scrolling.
pub fn terminal_write_bytes(bytes: &[u8]) {
    let mut state = CONSOLE.lock();
    if !state.fb.is_present() {
        return;
    }

    for &byte in bytes {
        terminal_write_byte_locked(&mut state, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::BootFramebufferFormat;

    #[test]
    fn encode_rgb_tracks_pixel_order() {
        let rgb = super::Framebuffer {
            virt_addr: 1,
            width: 1,
            height: 1,
            stride: 1,
            bpp: 32,
            format: BootFramebufferFormat::Rgbx,
            size_bytes: 4,
        };
        let bgr = super::Framebuffer {
            format: BootFramebufferFormat::Bgrx,
            ..rgb
        };

        assert_eq!(rgb.encode_rgb(0x11, 0x22, 0x33), 0x0011_2233);
        assert_eq!(bgr.encode_rgb(0x11, 0x22, 0x33), 0x0033_2211);
    }
}
