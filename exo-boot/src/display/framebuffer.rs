//! framebuffer.rs — Abstraction framebuffer pour le bootloader.
//!
//! Fournit :
//!   - `Framebuffer` : représente le framebuffer linéaire GOP.
//!   - `FramebufferFormat` : format pixel (Rgbx, Bgrx, …).
//!   - `PanicWriter` : écrit du texte rouge sur framebuffer (panic handler).
//!   - `BootWriter`  : écrit du texte blanc/cyan sur framebuffer (messages boot).
//!   - Fonctions utilitaires pour l'init, le clear screen, le dessin de texte.
//!
//! RÈGLE : Ce module ne fait aucune allocation dynamique.
//!   Toutes les structures sont sur la stack ou dans des statics.
//!
//! Dépendances :
//!   - `crate::display::font` : bitmap font PSF (8×16 pixels par glyphe).
//!   - `crate::kernel_loader::handoff::FramebufferInfo` : format de handoff.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spinning_top::Spinlock;

use super::font::{FONT_GLYPH_WIDTH, FONT_GLYPH_HEIGHT, glyph_for};
use crate::kernel_loader::handoff::{FramebufferInfo, PixelFormat};

// ─── FramebufferFormat ────────────────────────────────────────────────────────

/// Format des pixels dans le framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramebufferFormat {
    /// 32 bits : R8 G8 B8 X8 (X = ignoré).
    Rgbx,
    /// 32 bits : B8 G8 R8 X8 (X = ignoré).
    Bgrx,
    /// Format inconnu / non supporté — on écrit des patterns d'urgence.
    Unknown,
}

impl FramebufferFormat {
    /// Encode une couleur RGB en valeur 32 bits selon le format.
    #[inline]
    pub fn encode_rgb(self, r: u8, g: u8, b: u8) -> u32 {
        match self {
            Self::Rgbx => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
            Self::Bgrx => ((b as u32) << 16) | ((g as u32) << 8) | (r as u32),
            Self::Unknown => 0xFFFF_FFFF,
        }
    }
}

impl From<PixelFormat> for FramebufferFormat {
    fn from(pf: PixelFormat) -> Self {
        match pf {
            PixelFormat::Rgbx   => Self::Rgbx,
            PixelFormat::Bgrx   => Self::Bgrx,
            _                   => Self::Unknown,
        }
    }
}

// ─── Couleurs prédéfinies ─────────────────────────────────────────────────────

/// Palette de couleurs pour les messages boot.
pub struct Color;

impl Color {
    pub const WHITE:   (u8, u8, u8) = (255, 255, 255);
    pub const BLACK:   (u8, u8, u8) = (0,   0,   0  );
    pub const RED:     (u8, u8, u8) = (220, 50,  47 );
    pub const GREEN:   (u8, u8, u8) = (133, 153, 0  );
    pub const YELLOW:  (u8, u8, u8) = (181, 137, 0  );
    pub const CYAN:    (u8, u8, u8) = (42,  161, 152);
    pub const BLUE:    (u8, u8, u8) = (38,  139, 210);
    pub const MAGENTA: (u8, u8, u8) = (211, 54,  130);
    pub const GRAY:    (u8, u8, u8) = (88,  110, 117);
    /// Fond sombre Exo-OS (Solarized Dark background).
    pub const BACKGROUND: (u8, u8, u8) = (0, 43, 54);
}

// ─── Framebuffer ─────────────────────────────────────────────────────────────

/// Accès au framebuffer GOP linéaire.
///
/// Le framebuffer est en mémoire physique, identité-mappé pendant le boot.
/// Après ExitBootServices, reste accessible car le bootloader maintient le mapping.
pub struct Framebuffer {
    /// Adresse physique du début du framebuffer.
    pub phys_addr: u64,
    /// Largeur en pixels.
    pub width:      u32,
    /// Hauteur en pixels.
    pub height:     u32,
    /// Stride : pixels par ligne (peut être > width si padding).
    pub stride:     u32,
    /// Format des pixels.
    pub format:     FramebufferFormat,
    /// Taille totale en bytes.
    pub size_bytes: u64,
}

impl Framebuffer {
    /// Construit un Framebuffer "absent" (aucun écran disponible).
    /// Tous les accès seront ignorés car width/height == 0.
    #[inline]
    pub const fn absent() -> Self {
        Self {
            phys_addr:  0,
            width:      0,
            height:     0,
            stride:     0,
            format:     FramebufferFormat::Rgbx,
            size_bytes: 0,
        }
    }

    /// Retourne `true` si ce framebuffer est valide (width > 0 et adresse non nulle).
    #[inline]
    pub fn is_present(&self) -> bool {
        self.width > 0 && self.phys_addr != 0
    }

    /// Écrit un pixel à (x, y) avec la valeur encodée `pixel`.
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, pixel: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.stride + x) as usize * 4;
        let addr   = self.phys_addr as usize + offset;
        // SAFETY : L'offset est dans les limites du framebuffer (vérifié ci-dessus).
        unsafe { core::ptr::write_volatile(addr as *mut u32, pixel) };
    }

    /// Dessine un rectangle plein de couleur `pixel`.
    pub fn fill_rect(&self, x: u32, y: u32, w: u32, h: u32, pixel: u32) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for row in y..y_end {
            for col in x..x_end {
                self.put_pixel(col, row, pixel);
            }
        }
    }

    /// Efface l'écran avec la couleur de fond.
    pub fn clear(&self) {
        let bg = self.format.encode_rgb(
            Color::BACKGROUND.0, Color::BACKGROUND.1, Color::BACKGROUND.2,
        );
        self.fill_rect(0, 0, self.width, self.height, bg);
    }

    /// Dessine un glyphe de la police bitmap à (x, y) avec les couleurs fg/bg.
    pub fn draw_glyph(&self, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
        let glyph = glyph_for(ch);
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..FONT_GLYPH_WIDTH as u32 {
                let bit = (bits >> (FONT_GLYPH_WIDTH as u32 - 1 - col)) & 1;
                let pixel = if bit != 0 { fg } else { bg };
                self.put_pixel(x + col, y + row as u32, pixel);
            }
        }
    }

    /// Convertit en `FramebufferInfo` pour BootInfo.
    pub fn to_info(&self) -> FramebufferInfo {
        let format = match self.format {
            FramebufferFormat::Rgbx => PixelFormat::Rgbx,
            FramebufferFormat::Bgrx => PixelFormat::Bgrx,
            FramebufferFormat::Unknown => PixelFormat::Custom,
        };
        FramebufferInfo {
            phys_addr:  self.phys_addr,
            width:      self.width,
            height:     self.height,
            stride:     self.stride,
            bpp:        32,
            format,
            size_bytes: self.size_bytes,
        }
    }
}

// ─── Framebuffer global ───────────────────────────────────────────────────────

/// Indique si un framebuffer a été initialisé.
static FRAMEBUFFER_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Données du framebuffer global (phys_addr, width, height, stride, format).
/// Encodé comme:  [phys_addr u64][width u32][height u32][stride u32][format u32][size u64]
static FB_PHYS_ADDR:  AtomicU64 = AtomicU64::new(0);
static FB_WIDTH:      core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FB_HEIGHT:     core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FB_STRIDE:     core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FB_FORMAT_U32: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FB_SIZE:       AtomicU64 = AtomicU64::new(0);

/// Enregistre le framebuffer GOP globalement.
pub fn init_global_framebuffer(fb: &Framebuffer) {
    let fmt_u32 = match fb.format {
        FramebufferFormat::Rgbx => 0,
        FramebufferFormat::Bgrx => 1,
        FramebufferFormat::Unknown => 2,
    };
    FB_PHYS_ADDR.store(fb.phys_addr, Ordering::Release);
    FB_WIDTH.store(fb.width, Ordering::Release);
    FB_HEIGHT.store(fb.height, Ordering::Release);
    FB_STRIDE.store(fb.stride, Ordering::Release);
    FB_FORMAT_U32.store(fmt_u32, Ordering::Release);
    FB_SIZE.store(fb.size_bytes, Ordering::Release);
    FRAMEBUFFER_INITIALIZED.store(true, Ordering::Release);
}

/// Tente de récupérer le framebuffer global.
/// Retourne un `Framebuffer` construit depuis les atomics.
pub fn try_get_framebuffer() -> Option<Framebuffer> {
    if !FRAMEBUFFER_INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    let fmt = match FB_FORMAT_U32.load(Ordering::Relaxed) {
        0 => FramebufferFormat::Rgbx,
        1 => FramebufferFormat::Bgrx,
        _ => FramebufferFormat::Unknown,
    };
    Some(Framebuffer {
        phys_addr: FB_PHYS_ADDR.load(Ordering::Relaxed),
        width:      FB_WIDTH.load(Ordering::Relaxed),
        height:     FB_HEIGHT.load(Ordering::Relaxed),
        stride:     FB_STRIDE.load(Ordering::Relaxed),
        format:     fmt,
        size_bytes: FB_SIZE.load(Ordering::Relaxed),
    })
}

// ─── BootWriter — texte normal ────────────────────────────────────────────────

/// Position du curseur texte.
struct TextCursor {
    col: u32,
    row: u32,
}

impl TextCursor {
    const fn new() -> Self { Self { col: 0, row: 0 } }
}

/// Spinlock pour le curseur du BootWriter.
static BOOT_CURSOR: Spinlock<TextCursor> = Spinlock::new(TextCursor::new());

/// Implémentation de `fmt::Write` pour afficher du texte pendant le boot.
///
/// Utilise la police bitmap 8×16 et les couleurs Cyan/fond sombre.
pub struct BootWriter;

impl fmt::Write for BootWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let fb = match try_get_framebuffer() {
            Some(f) => f,
            None    => return Ok(()), // Pas de framebuffer — silencieux
        };

        let fg = fb.format.encode_rgb(Color::CYAN.0, Color::CYAN.1, Color::CYAN.2);
        let bg = fb.format.encode_rgb(Color::BACKGROUND.0, Color::BACKGROUND.1, Color::BACKGROUND.2);

        let mut cursor = BOOT_CURSOR.lock();
        let cols = fb.width / FONT_GLYPH_WIDTH as u32;
        let rows = fb.height / FONT_GLYPH_HEIGHT as u32;

        for ch in s.bytes() {
            match ch {
                b'\n' => {
                    cursor.col = 0;
                    cursor.row += 1;
                }
                b'\r' => { cursor.col = 0; }
                b'\t' => {
                    let next_tab = (cursor.col + 4) & !3;
                    cursor.col = next_tab.min(cols.saturating_sub(1));
                }
                _ => {
                    let x = cursor.col * FONT_GLYPH_WIDTH as u32;
                    let y = cursor.row * FONT_GLYPH_HEIGHT as u32;
                    fb.draw_glyph(x, y, ch, fg, bg);
                    cursor.col += 1;
                    if cursor.col >= cols {
                        cursor.col = 0;
                        cursor.row += 1;
                    }
                }
            }

            // Scroll si dernière ligne atteinte
            if cursor.row >= rows {
                scroll_framebuffer(&fb, bg);
                cursor.row = rows - 1;
            }
        }

        Ok(())
    }
}

// ─── PanicWriter — texte rouge panic ─────────────────────────────────────────

/// Spinlock pour le curseur du PanicWriter.
static PANIC_CURSOR: Spinlock<TextCursor> = Spinlock::new(TextCursor::new());

/// Implémentation de `fmt::Write` pour le panic handler.
/// Écrit en rouge sur fond noir pour maximiser la visibilité.
pub struct PanicWriter;

impl fmt::Write for PanicWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let fb = match try_get_framebuffer() {
            Some(f) => f,
            None    => return Ok(()),
        };

        // Fond noir pour le panic
        let fg = fb.format.encode_rgb(Color::RED.0, Color::RED.1, Color::RED.2);
        let bg = fb.format.encode_rgb(0, 0, 0);

        let mut cursor = PANIC_CURSOR.lock();
        let cols = fb.width / FONT_GLYPH_WIDTH as u32;
        let rows = fb.height / FONT_GLYPH_HEIGHT as u32;

        for ch in s.bytes() {
            match ch {
                b'\n' => { cursor.col = 0; cursor.row += 1; }
                b'\r' => { cursor.col = 0; }
                _ => {
                    let x = cursor.col * FONT_GLYPH_WIDTH as u32;
                    let y = cursor.row * FONT_GLYPH_HEIGHT as u32;
                    fb.draw_glyph(x, y, ch, fg, bg);
                    cursor.col += 1;
                    if cursor.col >= cols {
                        cursor.col = 0;
                        cursor.row += 1;
                    }
                }
            }
            if cursor.row >= rows {
                scroll_framebuffer(&fb, bg);
                cursor.row = rows - 1;
            }
        }

        Ok(())
    }
}

/// Initialise le PanicWriter (efface toute la moitié inférieure de l'écran en noir).
pub fn init_panic_display(fb: &Framebuffer) {
    let bg = fb.format.encode_rgb(0, 0, 0);
    let half_height = fb.height / 2;
    fb.fill_rect(0, half_height, fb.width, half_height, bg);

    // Remet le curseur panic en haut de la zone panic
    let rows = half_height / FONT_GLYPH_HEIGHT as u32;
    let _ = rows; // Utilisé pour positionner
    *PANIC_CURSOR.lock() = TextCursor { col: 0, row: half_height / FONT_GLYPH_HEIGHT as u32 };
}

// ─── Scroll framebuffer ───────────────────────────────────────────────────────

/// Fait défiler le framebuffer d'une ligne vers le haut.
/// Copie les pixels ligne par ligne de bas en haut.
fn scroll_framebuffer(fb: &Framebuffer, clear_color: u32) {
    let bytes_per_row = fb.stride as usize * 4;
    let glyph_h = FONT_GLYPH_HEIGHT as usize;
    let scroll_pixels = glyph_h; // Défile d'une ligne texte

    // Déplace chaque ligne d'une ligne de glyphe vers le haut
    for y in 0..(fb.height as usize - scroll_pixels) {
        let src_row = (y + scroll_pixels) * bytes_per_row;
        let dst_row = y * bytes_per_row;
        unsafe {
            let src = (fb.phys_addr as usize + src_row) as *const u8;
            let dst = (fb.phys_addr as usize + dst_row) as *mut u8;
            core::ptr::copy(src, dst, bytes_per_row);
        }
    }

    // Efface la dernière ligne
    let last_y = (fb.height as usize - scroll_pixels) as u32;
    fb.fill_rect(0, last_y, fb.width, scroll_pixels as u32, clear_color);
}

// ─── Barre de progression boot ────────────────────────────────────────────────

/// Dessine une barre de progression en bas de l'écran.
///
/// `progress` : 0‥=100 (pourcentage).
pub fn draw_progress_bar(fb: &Framebuffer, progress: u8) {
    let bar_h    = 8u32;
    let bar_y    = fb.height.saturating_sub(bar_h + 4);
    let bar_x    = 20u32;
    let bar_w    = fb.width.saturating_sub(40);
    let fill_w   = (bar_w as u64 * progress.min(100) as u64 / 100) as u32;

    // Fond de la barre
    let bg = fb.format.encode_rgb(20, 20, 20);
    fb.fill_rect(bar_x, bar_y, bar_w, bar_h, bg);

    // Remplissage progressif
    let fg = fb.format.encode_rgb(Color::CYAN.0, Color::CYAN.1, Color::CYAN.2);
    if fill_w > 0 {
        fb.fill_rect(bar_x, bar_y, fill_w, bar_h, fg);
    }
}

/// Affiche le logo "Exo-OS" centré en haut de l'écran (ASCII art, 5 lignes).
pub fn draw_boot_logo(fb: &Framebuffer) {
    const LOGO: &[&[u8]] = &[
        b" ___                 ___  ____  ",
        b"|  _| _  _____     / _ \\/ ___| ",
        b"| |_ \\ \\/ / _ \\___| | | \\___ \\ ",
        b"|  _| >  < (_) |___| |_| |___) |",
        b"|___//_/\\_\\___/     \\___/|____/ ",
    ];

    let fg = fb.format.encode_rgb(Color::CYAN.0, Color::CYAN.1, Color::CYAN.2);
    let bg = fb.format.encode_rgb(Color::BACKGROUND.0, Color::BACKGROUND.1, Color::BACKGROUND.2);

    let start_row = 2u32; // 2 lignes de marge en haut
    for (line_idx, &line) in LOGO.iter().enumerate() {
        let y = (start_row + line_idx as u32) * FONT_GLYPH_HEIGHT as u32;
        let start_col = fb.width.saturating_sub(line.len() as u32 * FONT_GLYPH_WIDTH as u32) / 2;
        for (col_idx, &ch) in line.iter().enumerate() {
            let x = start_col + col_idx as u32 * FONT_GLYPH_WIDTH as u32;
            fb.draw_glyph(x, y, ch, fg, bg);
        }
    }
}
