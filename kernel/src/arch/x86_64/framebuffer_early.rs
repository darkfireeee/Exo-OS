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
use crate::memory::core::{
    align_up, Frame, PageFlags, PhysAddr, VirtAddr, FIXMAP_BASE, PAGE_SIZE,
};
use crate::memory::virt::address_space::kernel::KERNEL_AS;

use super::boot::early_init::{BootFramebufferFormat, BootInfo};

#[path = "../../../../exo-boot/src/display/font.rs"]
mod shared_font;

use shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};

const FRAMEBUFFER_WINDOW_OFFSET: u64 = 0x0200_0000;
const FRAMEBUFFER_VIRT_BASE: u64 = FIXMAP_BASE.as_u64() + FRAMEBUFFER_WINDOW_OFFSET;
const TITLE_SCALE: u32 = 4;
const SUBTITLE_SCALE: u32 = 2;
const BODY_SCALE: u32 = 1;
const STAGE_START_Y: u32 = 320;
const STAGE_ROW_HEIGHT: u32 = 28;

#[derive(Debug, Clone, Copy)]
struct Framebuffer {
    virt_addr:  u64,
    width:      u32,
    height:     u32,
    stride:     u32,
    bpp:        u32,
    format:     BootFramebufferFormat,
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
}

impl ConsoleState {
    const fn new() -> Self {
        Self {
            fb: Framebuffer::absent(),
            stage_count: 0,
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
        | PageFlags::NO_CACHE
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

    Some(FRAMEBUFFER_VIRT_BASE + page_off)
}

fn draw_logo(fb: &Framebuffer) {
    let fg = accent_color(fb);
    let fg_dark = accent_dark(fb);
    let bg = bg_color(fb);
    let center_x = fb.width / 2;
    let top_y = 58u32;
    let icon_size = 54i32;
    let cx = center_x as i32 - 205;
    let cy = top_y as i32 + 44;

    fb.draw_line(cx, cy - icon_size, cx + icon_size, cy, fg);
    fb.draw_line(cx + icon_size, cy, cx, cy + icon_size, fg);
    fb.draw_line(cx, cy + icon_size, cx - icon_size, cy, fg);
    fb.draw_line(cx - icon_size, cy, cx, cy - icon_size, fg);
    fb.draw_line(cx - 30, cy - 30, cx + 30, cy + 30, fg_dark);
    fb.draw_line(cx + 30, cy - 30, cx - 30, cy + 30, fg_dark);

    fb.draw_text_scaled(center_x - 120, top_y, "EXO-OS", TITLE_SCALE, fg, bg);
    fb.draw_text_centered(top_y + 88, "FRAMEBUFFER BOOT PATH ACTIVE", SUBTITLE_SCALE, text_color(fb), bg);
    fb.draw_text_centered(
        top_y + 124,
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

    fb.fill_rect(left, y, 760, 22, panel);
    fb.fill_rect(left, y + 21, 760, 1, accent_dark(fb));
    fb.draw_text_scaled(left + 14, y + 3, label, BODY_SCALE, fg, panel);
    fb.draw_text_scaled(left + 470, y + 3, "................", BODY_SCALE, muted, panel);
    fb.fill_rect(left + 620, y + 3, 120, 16, chip);
    fb.draw_text_centered(
        y + 3,
        "",
        BODY_SCALE,
        chip_text,
        chip,
    );
    fb.draw_text_scaled(left + 650, y + 3, status, BODY_SCALE, chip_text, chip);
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
        STAGE_START_Y - 36,
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
        let left = fb.width.saturating_sub(760) / 2;
        fb.fill_rect(left, y, 760, 22, panel_color(fb));
        fb.fill_rect(left, y + 21, 760, 1, accent_dark(fb));
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
    let fg = text_color(&fb);
    let ok = ok_color(&fb);
    let y = fb.height.saturating_sub(84);
    let x = fb.width.saturating_sub(820) / 2;

    fb.fill_rect(x, y, 820, 46, panel);
    fb.fill_rect(x, y, 820, 2, accent_color(&fb));
    fb.draw_text_centered(
        y + 10,
        "EXO-OS KERNEL BOOT COMPLETE",
        SUBTITLE_SCALE,
        ok,
        panel,
    );
    fb.draw_text_centered(
        y + 56,
        "Scheduler idle loop active - QEMU framebuffer proof complete",
        BODY_SCALE,
        fg,
        bg_color(&fb),
    );
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
