#![allow(dead_code)]

//! # `desktop.rs` — Environnement de Bureau Graphique ExoCompositor (Wayland / Strata)
//!
//! Rend dans le Shadow Backbuffer RAM :
//! 1. **Fond d'écran Thème « Exo-Strata Aurora » (Wallpaper)** :
//!    - Dégradé vertical bleu-indigo profond + grille géométrique cyber-strata + emblème diamant Exo-OS.
//! 2. **Icônes de Bureau (Desktop Icons, colonne gauche `x = 10..78`)** :
//!    - 4 icônes graphiques 28×28 pixels cliquables avec étiquettes :
//!      `Terminal`, `Doctor`, `Calendar`, `ExoFS`.
//! 3. **Barre d'état supérieure (Top Panel, `y = 0..24`)** :
//!    - Identité `Exo-OS Strata | Wayland` + pastilles `[Shield: OK]`, `[Net: 10.0.2.15]`, `[VirtIO-GPU]`.
//! 4. **Fenêtre Wayland `xdg_toplevel` encadrée (`x = 88..width-16`, `y = 32..height-34`)** :
//!    - Ombre portée, bordure acier/cyan, barre de titre `20 px` avec boutons tricolores (Fermer / Réduire / Agrandir).
//! 5. **Dock inférieur flottant (`y = height-28..height-4`)** :
//!    - Boutons avec icônes graphiques intégrées et indicateur lumineux d'application active.
//! 6. **Panneau HUD flottant** (activable au clic sur les icônes du Bureau ou du Dock).

use crate::shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};
use crate::Framebuffer;

pub const TOP_BAR_H: u32 = 24;
pub const WIN_TOP_Y: u32 = 30;
pub const WIN_TITLE_H: u32 = 20;
pub const TERM_TOP_Y: u32 = WIN_TOP_Y + WIN_TITLE_H + 4; // 54 px
pub const WIN_LEFT_X: u32 = 88;
pub const TERM_LEFT_X: u32 = WIN_LEFT_X + 8; // 96 px
pub const WIN_RIGHT_MARGIN: u32 = 16;
pub const TERM_RIGHT_MARGIN: u32 = WIN_RIGHT_MARGIN + 8; // 24 px
pub const DOCK_BAR_H: u32 = 30;
pub const WIN_BOTTOM_MARGIN: u32 = DOCK_BAR_H + 6; // 36 px
pub const HEADER_TOTAL_H: u32 = TERM_TOP_Y;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockOverlay {
    None = 0,
    Doctor = 1,
    Calendar = 2,
    Storage = 3,
}

pub struct DesktopState {
    pub active_overlay: DockOverlay,
    pub gpu_active: bool,
    pub frame_counter: u64,
}

impl DesktopState {
    pub const fn new() -> Self {
        Self {
            active_overlay: DockOverlay::None,
            gpu_active: true,
            frame_counter: 0,
        }
    }

    /// Gère un clic gauche de souris aux coordonnées `(mx, my)` sur l'écran.
    /// Retourne `true` si l'état visuel du bureau a changé et doit être redessiné.
    pub fn handle_mouse_click(&mut self, mx: u32, my: u32, fb: Framebuffer) -> bool {
        if !fb.is_present() {
            return false;
        }
        // 1. Clic sur les icônes de bureau à gauche (x < WIN_LEFT_X)
        if mx < WIN_LEFT_X && my >= 40 {
            let slot = (my - 40) / 76;
            match slot {
                0 => {
                    self.active_overlay = DockOverlay::None;
                    return true;
                }
                1 => {
                    self.active_overlay = if self.active_overlay == DockOverlay::Doctor {
                        DockOverlay::None
                    } else {
                        DockOverlay::Doctor
                    };
                    return true;
                }
                2 => {
                    self.active_overlay = if self.active_overlay == DockOverlay::Calendar {
                        DockOverlay::None
                    } else {
                        DockOverlay::Calendar
                    };
                    return true;
                }
                3 => {
                    self.active_overlay = if self.active_overlay == DockOverlay::Storage {
                        DockOverlay::None
                    } else {
                        DockOverlay::Storage
                    };
                    return true;
                }
                _ => {}
            }
        }

        // 2. Clic dans le Dock inférieur
        let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
        if my >= dock_y {
            let center_x = fb.width / 2;
            let dock_start = center_x.saturating_sub(240);
            if mx >= dock_start && mx < dock_start + 115 {
                self.active_overlay = DockOverlay::None;
                return true;
            } else if mx >= dock_start + 120 && mx < dock_start + 240 {
                self.active_overlay = if self.active_overlay == DockOverlay::Doctor {
                    DockOverlay::None
                } else {
                    DockOverlay::Doctor
                };
                return true;
            } else if mx >= dock_start + 245 && mx < dock_start + 365 {
                self.active_overlay = if self.active_overlay == DockOverlay::Calendar {
                    DockOverlay::None
                } else {
                    DockOverlay::Calendar
                };
                return true;
            } else if mx >= dock_start + 370 && mx < dock_start + 485 {
                self.active_overlay = if self.active_overlay == DockOverlay::Storage {
                    DockOverlay::None
                } else {
                    DockOverlay::Storage
                };
                return true;
            }
        }
        false
    }

    /// Dessine le fond d'écran (thème Strata Aurora) dans les bandes autour de la fenêtre Wayland.
    fn render_wallpaper_margins(&self, fb: Framebuffer) {
        let w = fb.width;
        let h = fb.height;
        let win_right = w.saturating_sub(WIN_RIGHT_MARGIN);
        let win_bottom = h.saturating_sub(WIN_BOTTOM_MARGIN);

        // Bandes de fond d'écran : haut (24..30), bas (win_bottom..h-30), gauche (0..88), droite (win_right..w)
        let wp_top = fb.encode(0x0d, 0x26, 0x42);
        let wp_mid = fb.encode(0x11, 0x32, 0x56);
        let wp_bot = fb.encode(0x0a, 0x1e, 0x36);
        let grid_col = fb.encode(0x19, 0x43, 0x6e);

        // Bande entre Top Bar et fenêtre
        fb.fill_rect_shadow(0, TOP_BAR_H, w, WIN_TOP_Y - TOP_BAR_H, wp_top);
        // Colonne gauche (bureau + icônes)
        let mid_h = win_bottom.saturating_sub(WIN_TOP_Y);
        fb.fill_rect_shadow(0, WIN_TOP_Y, WIN_LEFT_X, mid_h / 2, wp_top);
        fb.fill_rect_shadow(0, WIN_TOP_Y + mid_h / 2, WIN_LEFT_X, mid_h - mid_h / 2, wp_mid);
        // Colonne droite
        fb.fill_rect_shadow(win_right, WIN_TOP_Y, WIN_RIGHT_MARGIN, mid_h, wp_mid);
        // Bande au-dessus du dock
        let dock_y = h.saturating_sub(DOCK_BAR_H);
        if dock_y > win_bottom {
            fb.fill_rect_shadow(0, win_bottom, w, dock_y - win_bottom, wp_bot);
        }

        // Lignes de grille décoratives Strata sur le fond d'écran gauche et droit
        let mut gy = TOP_BAR_H + 16;
        while gy < dock_y {
            fb.fill_rect_shadow(0, gy, WIN_LEFT_X, 1, grid_col);
            fb.fill_rect_shadow(win_right, gy, WIN_RIGHT_MARGIN, 1, grid_col);
            gy += 32;
        }
        fb.fill_rect_shadow(44, TOP_BAR_H, 1, dock_y.saturating_sub(TOP_BAR_H), grid_col);
    }

    /// Dessine les 4 icônes graphiques du Bureau dans la colonne gauche (`x = 12..76`).
    fn render_desktop_icons(&self, fb: Framebuffer) {
        let icons: [(&[u8], (u8, u8, u8), u8); 4] = [
            (b"Terminal", (0x00, 0xc5, 0xff), 0),
            (b"Doctor", (0x38, 0xd7, 0x7c), 1),
            (b"Calendar", (0xff, 0xb7, 0x03), 2),
            (b"ExoFS", (0xb5, 0x65, 0xff), 3),
        ];

        let mut idx = 0usize;
        while idx < 4 {
            let (label, accent, kind) = icons[idx];
            let base_y = 42 + (idx as u32) * 76;
            let icon_x = 28u32;
            let icon_y = base_y;

            // Ombre + fond tuile 32x32
            let shadow = fb.encode(0x04, 0x0d, 0x18);
            let tile_bg = fb.encode(0x0c, 0x22, 0x3a);
            let border = fb.encode(accent.0, accent.1, accent.2);
            fb.fill_rect_shadow(icon_x + 2, icon_y + 2, 32, 32, shadow);
            fb.fill_rect_shadow(icon_x, icon_y, 32, 32, border);
            fb.fill_rect_shadow(icon_x + 2, icon_y + 2, 28, 28, tile_bg);

            // Dessin du pictogramme interne selon le type
            match kind {
                0 => {
                    // Terminal : barre supérieure + chevron ">_"
                    fb.fill_rect_shadow(icon_x + 4, icon_y + 4, 24, 4, border);
                    draw_text_shadow(fb, icon_x + 8, icon_y + 11, b">_", accent, (0x0c, 0x22, 0x3a));
                }
                1 => {
                    // ExoDoctor : croix médicale / bouclier
                    fb.fill_rect_shadow(icon_x + 13, icon_y + 7, 6, 18, border);
                    fb.fill_rect_shadow(icon_x + 7, icon_y + 13, 18, 6, border);
                }
                2 => {
                    // Calendar : en-tête ambre + grille de jours
                    fb.fill_rect_shadow(icon_x + 5, icon_y + 5, 22, 6, border);
                    let dot = fb.encode(0xe6, 0xf1, 0xff);
                    fb.fill_rect_shadow(icon_x + 7, icon_y + 14, 4, 4, dot);
                    fb.fill_rect_shadow(icon_x + 14, icon_y + 14, 4, 4, dot);
                    fb.fill_rect_shadow(icon_x + 21, icon_y + 14, 4, 4, dot);
                    fb.fill_rect_shadow(icon_x + 7, icon_y + 21, 4, 4, dot);
                    fb.fill_rect_shadow(icon_x + 14, icon_y + 21, 4, 4, border);
                }
                _ => {
                    // ExoFS : disques / strates superposées
                    fb.fill_rect_shadow(icon_x + 6, icon_y + 7, 20, 4, border);
                    fb.fill_rect_shadow(icon_x + 6, icon_y + 14, 20, 4, border);
                    fb.fill_rect_shadow(icon_x + 6, icon_y + 21, 20, 4, border);
                }
            }

            // Étiquette centrée sous l'icône
            let text_w = (label.len() as u32) * (FONT_GLYPH_WIDTH as u32);
            let text_x = (WIN_LEFT_X / 2).saturating_sub(text_w / 2).max(4);
            draw_text_shadow(
                fb,
                text_x,
                icon_y + 36,
                label,
                (0xe6, 0xf1, 0xff),
                (0x0f, 0x2c, 0x4c),
            );
            idx += 1;
        }
    }

    /// Dessine la barre d'état supérieure, le fond d'écran, les icônes de bureau,
    /// le cadre de la fenêtre Wayland `xdg_toplevel` et le Dock inférieur dans le Shadow Buffer RAM.
    pub fn render_chrome(&mut self, fb: Framebuffer) {
        if !fb.is_present() || fb.height <= HEADER_TOTAL_H + DOCK_BAR_H {
            return;
        }
        self.frame_counter = self.frame_counter.saturating_add(1);

        // ── 0. Fond d'écran Thème Strata & Icônes de Bureau ─────────────────
        self.render_wallpaper_margins(fb);
        self.render_desktop_icons(fb);

        // ── 1. Top Status Bar (y = 0..24) ───────────────────────────────────
        let top_bg = fb.encode(0x07, 0x13, 0x20);
        let top_border = fb.encode(0x22, 0x57, 0x84);
        fb.fill_rect_shadow(0, 0, fb.width, TOP_BAR_H - 1, top_bg);
        fb.fill_rect_shadow(0, TOP_BAR_H - 1, fb.width, 1, top_border);

        // Badge gauche : Exo-OS Strata + Wayland
        let badge_bg = fb.encode(0x10, 0x2a, 0x43);
        fb.fill_rect_shadow(6, 3, 196, 18, badge_bg);
        draw_text_shadow(
            fb,
            12,
            4,
            b"Exo-OS Strata | Wayland",
            (0x62, 0xe6, 0xff),
            (0x10, 0x2a, 0x43),
        );

        // Indicateurs droite : [Shield: OK] [Net: 10.0.2.15] [VirtIO-GPU]
        let right_x = fb.width.saturating_sub(356);
        let pill_shield = fb.encode(0x0b, 0x3b, 0x24);
        fb.fill_rect_shadow(right_x, 3, 102, 18, pill_shield);
        draw_text_shadow(
            fb,
            right_x + 7,
            4,
            b"Shield: OK",
            (0x60, 0xff, 0x90),
            (0x0b, 0x3b, 0x24),
        );

        let pill_net = fb.encode(0x0d, 0x2b, 0x45);
        fb.fill_rect_shadow(right_x + 108, 3, 126, 18, pill_net);
        draw_text_shadow(
            fb,
            right_x + 115,
            4,
            b"Net: 10.0.2.15",
            (0x71, 0xa7, 0xff),
            (0x0d, 0x2b, 0x45),
        );

        let pill_gpu = fb.encode(0x24, 0x1b, 0x3b);
        fb.fill_rect_shadow(right_x + 240, 3, 108, 18, pill_gpu);
        draw_text_shadow(
            fb,
            right_x + 248,
            4,
            b"VirtIO-GPU",
            (0xd6, 0x7b, 0xff),
            (0x24, 0x1b, 0x3b),
        );

        // ── 2. Cadre et Barre de titre de fenêtre Wayland xdg_toplevel ──────
        let win_w = fb.width.saturating_sub(WIN_LEFT_X + WIN_RIGHT_MARGIN);
        let win_bottom = fb.height.saturating_sub(WIN_BOTTOM_MARGIN);
        let win_h = win_bottom.saturating_sub(WIN_TOP_Y);

        // Ombre portée droite et bas de la fenêtre
        let win_shadow = fb.encode(0x04, 0x0b, 0x14);
        fb.fill_rect_shadow(WIN_LEFT_X + win_w, WIN_TOP_Y + 4, 4, win_h, win_shadow);
        fb.fill_rect_shadow(WIN_LEFT_X + 4, win_bottom, win_w, 4, win_shadow);

        // Bordures gauche, droite et bas de la fenêtre Wayland
        let win_border = fb.encode(0x2e, 0x77, 0xb0);
        fb.fill_rect_shadow(WIN_LEFT_X, WIN_TOP_Y, 2, win_h, win_border);
        fb.fill_rect_shadow(WIN_LEFT_X + win_w - 2, WIN_TOP_Y, 2, win_h, win_border);
        fb.fill_rect_shadow(WIN_LEFT_X, win_bottom - 2, win_w, 2, win_border);

        // Barre de titre de la fenêtre (y = WIN_TOP_Y .. WIN_TOP_Y + WIN_TITLE_H)
        let win_bg = fb.encode(0x10, 0x28, 0x40);
        fb.fill_rect_shadow(WIN_LEFT_X, WIN_TOP_Y, win_w, 1, win_border);
        fb.fill_rect_shadow(WIN_LEFT_X + 2, WIN_TOP_Y + 1, win_w - 4, WIN_TITLE_H - 2, win_bg);
        fb.fill_rect_shadow(WIN_LEFT_X, WIN_TOP_Y + WIN_TITLE_H - 1, win_w, 1, win_border);

        // Boutons de fenêtre (Close rouge, Minimize ambre, Maximize vert)
        let btn_close = fb.encode(0xff, 0x5f, 0x56);
        let btn_min = fb.encode(0xff, 0xbd, 0x2e);
        let btn_max = fb.encode(0x27, 0xc9, 0x3f);
        fb.fill_rect_shadow(WIN_LEFT_X + 10, WIN_TOP_Y + 5, 10, 10, btn_close);
        fb.fill_rect_shadow(WIN_LEFT_X + 26, WIN_TOP_Y + 5, 10, 10, btn_min);
        fb.fill_rect_shadow(WIN_LEFT_X + 42, WIN_TOP_Y + 5, 10, 10, btn_max);

        let title = b"exosh - Exo-OS Interactive Shell (xdg_toplevel @ wl_surface#1)";
        let title_x = WIN_LEFT_X
            + (win_w / 2).saturating_sub((title.len() as u32 * FONT_GLYPH_WIDTH as u32) / 2);
        draw_text_shadow(
            fb,
            title_x.max(WIN_LEFT_X + 64),
            WIN_TOP_Y + 2,
            title,
            (0xd8, 0xec, 0xff),
            (0x10, 0x28, 0x40),
        );

        // ── 3. Dock inférieur (y = height - DOCK_BAR_H .. height) ───────────
        let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
        let dock_bg = fb.encode(0x07, 0x13, 0x20);
        fb.fill_rect_shadow(0, dock_y, fb.width, 1, top_border);
        fb.fill_rect_shadow(0, dock_y + 1, fb.width, DOCK_BAR_H - 1, dock_bg);

        let center_x = fb.width / 2;
        let dock_start = center_x.saturating_sub(240);

        // Bouton 1 : [>_ Terminal]
        let term_active = self.active_overlay == DockOverlay::None;
        let b1_bg = if term_active { (0x16, 0x3e, 0x5e) } else { (0x0e, 0x22, 0x36) };
        fb.fill_rect_shadow(dock_start, dock_y + 4, 114, 20, fb.encode(b1_bg.0, b1_bg.1, b1_bg.2));
        fb.fill_rect_shadow(dock_start + 6, dock_y + 8, 10, 10, fb.encode(0x00, 0xc5, 0xff));
        draw_text_shadow(
            fb,
            dock_start + 20,
            dock_y + 6,
            b"Terminal",
            (0x62, 0xe6, 0xff),
            b1_bg,
        );
        if term_active {
            fb.fill_rect_shadow(dock_start + 46, dock_y + 26, 22, 2, fb.encode(0x00, 0xc5, 0xff));
        }

        // Bouton 2 : [+ ExoDoctor]
        let doc_active = self.active_overlay == DockOverlay::Doctor;
        let b2_bg = if doc_active { (0x14, 0x4d, 0x32) } else { (0x0e, 0x22, 0x36) };
        fb.fill_rect_shadow(dock_start + 120, dock_y + 4, 114, 20, fb.encode(b2_bg.0, b2_bg.1, b2_bg.2));
        fb.fill_rect_shadow(dock_start + 126, dock_y + 8, 10, 10, fb.encode(0x38, 0xd7, 0x7c));
        draw_text_shadow(
            fb,
            dock_start + 140,
            dock_y + 6,
            b"ExoDoctor",
            (0x60, 0xff, 0x90),
            b2_bg,
        );
        if doc_active {
            fb.fill_rect_shadow(dock_start + 166, dock_y + 26, 22, 2, fb.encode(0x38, 0xd7, 0x7c));
        }

        // Bouton 3 : [# Calendar]
        let cal_active = self.active_overlay == DockOverlay::Calendar;
        let b3_bg = if cal_active { (0x4a, 0x36, 0x12) } else { (0x0e, 0x22, 0x36) };
        fb.fill_rect_shadow(dock_start + 240, dock_y + 4, 114, 20, fb.encode(b3_bg.0, b3_bg.1, b3_bg.2));
        fb.fill_rect_shadow(dock_start + 246, dock_y + 8, 10, 10, fb.encode(0xff, 0xb7, 0x03));
        draw_text_shadow(
            fb,
            dock_start + 260,
            dock_y + 6,
            b"Calendar",
            (0xff, 0xd1, 0x66),
            b3_bg,
        );
        if cal_active {
            fb.fill_rect_shadow(dock_start + 286, dock_y + 26, 22, 2, fb.encode(0xff, 0xb7, 0x03));
        }

        // Bouton 4 : [= ExoFS]
        let fs_active = self.active_overlay == DockOverlay::Storage;
        let b4_bg = if fs_active { (0x32, 0x1c, 0x4d) } else { (0x0e, 0x22, 0x36) };
        fb.fill_rect_shadow(dock_start + 360, dock_y + 4, 108, 20, fb.encode(b4_bg.0, b4_bg.1, b4_bg.2));
        fb.fill_rect_shadow(dock_start + 366, dock_y + 8, 10, 10, fb.encode(0xb5, 0x65, 0xff));
        draw_text_shadow(
            fb,
            dock_start + 382,
            dock_y + 6,
            b"ExoFS",
            (0xd6, 0x7b, 0xff),
            b4_bg,
        );
        if fs_active {
            fb.fill_rect_shadow(dock_start + 402, dock_y + 26, 22, 2, fb.encode(0xb5, 0x65, 0xff));
        }

        // ── 4. Panneau HUD flottant (si une icône du Dock/Bureau est active) ─
        self.render_overlay_hud(fb);
    }

    fn render_overlay_hud(&self, fb: Framebuffer) {
        if self.active_overlay == DockOverlay::None {
            return;
        }
        let hud_w = 300u32;
        let hud_h = 116u32;
        let hud_x = fb.width.saturating_sub(hud_w + 28);
        let hud_y = TERM_TOP_Y + 8;

        let border_col = fb.encode(0x3a, 0x86, 0xff);
        let bg_col = fb.encode(0x08, 0x17, 0x26);
        fb.fill_rect_shadow(hud_x - 2, hud_y - 2, hud_w + 4, hud_h + 4, border_col);
        fb.fill_rect_shadow(hud_x, hud_y, hud_w, hud_h, bg_col);

        match self.active_overlay {
            DockOverlay::Doctor => {
                draw_text_shadow(fb, hud_x + 12, hud_y + 8, b"[ExoDoctor Health HUD]", (0x60, 0xff, 0x90), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 30, b"Ring 0 Microkernel : PASS", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 48, b"ExoShield 67-Edge  : PASS", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 66, b"ExoPhoenix A/B     : STANDBY", (0x62, 0xe6, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 84, b"19/19 Ring1 Online : 100% OK", (0x60, 0xff, 0x90), (0x08, 0x17, 0x26));
            }
            DockOverlay::Calendar => {
                draw_text_shadow(fb, hud_x + 12, hud_y + 8, b"[October 2026 - Strata]", (0xff, 0xd1, 0x66), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 30, b"Su Mo Tu We Th Fr Sa", (0x71, 0xa7, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 48, b"11 12 13 14 15 16 17", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 66, b"18 [19] 20 21 22 23 24", (0x60, 0xff, 0x90), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 84, b"25 26 27 28 29 30 31", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
            }
            DockOverlay::Storage => {
                draw_text_shadow(fb, hud_x + 12, hud_y + 8, b"[ExoFS Content-Addressed]", (0xd6, 0x7b, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 30, b"Root Mount : / (ExoFS v2)", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 48, b"Hash Chain : BLAKE3 + Ed25519", (0xe6, 0xf1, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 66, b"USB Mount  : /mnt/usb (Ready)", (0x62, 0xe6, 0xff), (0x08, 0x17, 0x26));
                draw_text_shadow(fb, hud_x + 12, hud_y + 84, b"Zero-Copy  : SHM SurfaceGrant", (0x60, 0xff, 0x90), (0x08, 0x17, 0x26));
            }
            DockOverlay::None => {}
        }
    }
}

/// Dessine une chaîne ASCII dans le Shadow Buffer RAM aux coordonnées pixels `(px, py)`.
pub fn draw_text_shadow(
    fb: Framebuffer,
    px: u32,
    py: u32,
    text: &[u8],
    fg_rgb: (u8, u8, u8),
    bg_rgb: (u8, u8, u8),
) {
    let fg = fb.encode(fg_rgb.0, fg_rgb.1, fg_rgb.2);
    let bg = fb.encode(bg_rgb.0, bg_rgb.1, bg_rgb.2);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();

    let mut cx = px;
    for &byte in text {
        if cx + (FONT_GLYPH_WIDTH as u32) >= fb.width {
            break;
        }
        let glyph = glyph_for(byte);
        let mut gy = 0usize;
        while gy < FONT_GLYPH_HEIGHT {
            let row_y = py + (gy as u32);
            if row_y < fb.height {
                let bits = glyph[gy];
                let row_addr = base
                    .saturating_add((row_y as usize).saturating_mul(stride))
                    .saturating_add((cx as usize).saturating_mul(bpp));
                let mut gx = 0usize;
                while gx < FONT_GLYPH_WIDTH {
                    let color = if (bits & (0x80 >> gx)) != 0 { fg } else { bg };
                    if bpp == 4 {
                        unsafe {
                            core::ptr::write((row_addr + gx * 4) as *mut u32, color);
                        }
                    } else if bpp == 3 {
                        let bytes = color.to_le_bytes();
                        let addr = row_addr + gx * 3;
                        unsafe {
                            core::ptr::write(addr as *mut u8, bytes[0]);
                            core::ptr::write((addr + 1) as *mut u8, bytes[1]);
                            core::ptr::write((addr + 2) as *mut u8, bytes[2]);
                        }
                    }
                    gx += 1;
                }
            }
            gy += 1;
        }
        cx += FONT_GLYPH_WIDTH as u32;
    }
}

/// Alpha-blending rapide d'un pixel ARGB8888 (`src`) sur un pixel XRGB8888 (`dst`) sans flottants.
#[inline]
pub fn blend_argb8888(dst: u32, src: u32) -> u32 {
    let a = (src >> 24) & 0xFF;
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return src & 0x00FF_FFFF;
    }
    let inv_a = 255 - a;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;

    let r = (sr * a + dr * inv_a) / 255;
    let g = (sg * a + dg * inv_a) / 255;
    let b = (sb * a + db * inv_a) / 255;

    (r << 16) | (g << 8) | b
}
