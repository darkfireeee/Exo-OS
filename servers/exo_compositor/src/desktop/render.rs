//! Layered desktop compositing pipeline, desktop icons, dev FPS HUD, and visual QA tests.

use crate::apps::files::TRASH_DIR;
use crate::apps::AppKind;
use crate::assets::{IconId, WALLPAPER_HARMONY_H, WALLPAPER_HARMONY_RGB565, WALLPAPER_HARMONY_W};
use crate::desktop::state::{DesktopShell, TOP_BAR_H};
use exo_ui::{Canvas, Color, Rect};

pub(crate) struct DeskShortcut {
    pub kind: AppKind,
    pub icon: IconId,
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub dir_override: Option<&'static str>,
}

pub(crate) const DESKTOP_SHORTCUTS: [DeskShortcut; 4] = [
    DeskShortcut { kind: AppKind::Files, icon: IconId::Files, label_fr: "Fichiers", label_en: "Files", dir_override: None },
    DeskShortcut { kind: AppKind::Files, icon: IconId::Trash, label_fr: "Corbeille", label_en: "Trash", dir_override: Some(TRASH_DIR) },
    DeskShortcut { kind: AppKind::Bench, icon: IconId::Bench, label_fr: "ExoBench", label_en: "ExoBench", dir_override: None },
    DeskShortcut { kind: AppKind::Games, icon: IconId::Games, label_fr: "Jeux", label_en: "Games", dir_override: None },
];

/// Exact non-overlapping rectangle subtraction for front-to-back occlusion culling.
pub(crate) fn subtract_rect(region: &mut Vec<Rect>, cut: Rect) {
    if cut.is_empty() || region.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(region.len() + 4);
    for &r in region.iter() {
        let inter = r.intersect(cut);
        if inter.is_empty() {
            out.push(r);
            continue;
        }
        if inter.y > r.y {
            out.push(Rect::new(r.x, r.y, r.w, inter.y - r.y));
        }
        if inter.bottom() < r.bottom() {
            out.push(Rect::new(r.x, inter.bottom(), r.w, r.bottom() - inter.bottom()));
        }
        if inter.x > r.x {
            out.push(Rect::new(r.x, inter.y, inter.x - r.x, inter.h));
        }
        if inter.right() < r.right() {
            out.push(Rect::new(inter.right(), inter.y, r.right() - inter.right(), inter.h));
        }
    }
    *region = out;
}

#[inline]
fn blit_wallpaper_rect(dst: &mut [u32], wallpaper: &[u32], row_px: usize, r: Rect) {
    if r.is_empty() {
        return;
    }
    let x0 = r.x as usize;
    let x1 = r.right() as usize;
    let y0 = r.y as usize;
    let y1 = r.bottom() as usize;
    if x0 == 0 && x1 == row_px {
        let s = y0.saturating_mul(row_px).min(dst.len()).min(wallpaper.len());
        let e = y1.saturating_mul(row_px).min(dst.len()).min(wallpaper.len());
        if s < e {
            dst[s..e].copy_from_slice(&wallpaper[s..e]);
        }
    } else {
        let len = dst.len().min(wallpaper.len());
        for y in y0..y1 {
            let row_off = y * row_px;
            let s = row_off + x0;
            let e = row_off + x1;
            if e <= len {
                dst[s..e].copy_from_slice(&wallpaper[s..e]);
            }
        }
    }
}

impl DesktopShell {
    #[inline]
    pub fn fps_hud_rect(&self) -> Rect {
        let w = 276.min(self.width as i32 - 16);
        Rect::new(self.width as i32 - w - 10, TOP_BAR_H + 8, w, 28)
    }

    /// Computes exact visible (unoccluded) sub-rectangles of `target` for `win_id`.
    /// Returns empty when `win_id` is completely buried behind higher opaque windows.
    pub(crate) fn visible_window_rects(&self, win_id: u32, target: Rect) -> Vec<Rect> {
        let screen = Rect::new(0, 0, self.width as i32, self.height as i32);
        let clipped = target.intersect(screen);
        if clipped.is_empty() {
            return Vec::new();
        }
        let Some(target_z) = self.windows.iter().find(|w| w.id == win_id && !w.minimized).map(|w| w.z_order) else {
            return Vec::new();
        };
        let mut vis = vec![clipped];
        if self.theme.surface_alpha >= 235 {
            for w in &self.windows {
                if !w.minimized && w.id != win_id && w.z_order > target_z {
                    subtract_rect(&mut vis, w.opaque_rect());
                    if vis.is_empty() {
                        break;
                    }
                }
            }
        }
        vis
    }

    pub(crate) fn desktop_icon_rect(idx: usize) -> Rect {
        Rect::new(20, TOP_BAR_H + 20 + (idx as i32) * 92, 84, 80)
    }

    fn ensure_wallpaper_cache(&mut self) {
        let total = (self.width as usize).saturating_mul(self.height as usize);
        if self.wallpaper_cache.len() == total && self.wallpaper_cache_idx == self.theme.wallpaper_idx {
            return;
        }
        self.wallpaper_cache.resize(total, 0xFF0E_1117);
        let mut cv = Canvas::new(&mut self.wallpaper_cache, self.width, self.height, self.width as usize);
        match self.theme.wallpaper_idx {
            0 => cv.blit_rgb565_wallpaper(WALLPAPER_HARMONY_RGB565, WALLPAPER_HARMONY_W, WALLPAPER_HARMONY_H),
            1 => cv.render_harmonious_wallpaper(1),
            2 => cv.render_harmonious_wallpaper(2),
            _ => cv.render_harmonious_wallpaper(3),
        }
        self.wallpaper_cache_idx = self.theme.wallpaper_idx;
    }

    #[cfg(test)]
    pub fn render(&mut self, pixels: &mut [u32]) {
        self.render_rect(pixels, Rect::new(0, 0, self.width as i32, self.height as i32));
    }

    /// Double-buffered present into `fb_server` SHM: renders 100% into private `self.backbuf`
    /// and copies only finalized scanlines into `shm_pixels`, preventing any mid-frame wallpaper flash.
    pub fn render_rect_to_shm(&mut self, shm_pixels: &mut [u32], dirty_rect: Rect) {
        let total = (self.width as usize).saturating_mul(self.height as usize);
        let screen = Rect::new(0, 0, self.width as i32, self.height as i32);
        let mut target_rect = dirty_rect.intersect(screen);
        let mut buf = core::mem::take(&mut self.backbuf);
        if buf.len() != total {
            buf.resize(total, 0xFF0E_1117);
            target_rect = screen;
        }
        if !target_rect.is_empty() {
            self.render_rect(&mut buf, target_rect);
            let row_px = self.width as usize;
            let x0 = target_rect.x as usize;
            let x1 = target_rect.right() as usize;
            let y0 = target_rect.y as usize;
            let y1 = target_rect.bottom() as usize;
            if x0 == 0 && x1 == row_px {
                let s = y0.saturating_mul(row_px).min(shm_pixels.len()).min(buf.len());
                let e = y1.saturating_mul(row_px).min(shm_pixels.len()).min(buf.len());
                if s < e {
                    shm_pixels[s..e].copy_from_slice(&buf[s..e]);
                }
            } else {
                for y in y0..y1 {
                    let row_off = y * row_px;
                    let s = row_off + x0;
                    let e = row_off + x1;
                    if e <= shm_pixels.len() && e <= buf.len() {
                        shm_pixels[s..e].copy_from_slice(&buf[s..e]);
                    }
                }
            }
        }
        self.backbuf = buf;
    }

    /// Clip-aware 2D dirty-rectangle compositor with per-window surface caching and opaque fast-path:
    /// skips wallpaper & background layers completely when only the focused opaque window body changed.
    pub fn render_rect(&mut self, pixels: &mut [u32], dirty_rect: Rect) {
        self.ensure_wallpaper_cache();
        let screen = Rect::new(0, 0, self.width as i32, self.height as i32);
        let band = dirty_rect.intersect(screen);
        if band.is_empty() {
            return;
        }
        let row_px = self.width as usize;

        // Opaque focused-window fast-path: when typing/pasting/scrolling strictly inside the topmost
        // opaque window body, skip wallpaper wipe, drop shadow, dock, and all lower windows!
        let fps_active = self.theme.show_fps || self.cfg.dev_mode;
        let fps_overlap = fps_active && !band.intersect(self.fps_hud_rect()).is_empty();
        let no_overlays = !self.locked
            && self.power_transition.is_none()
            && !self.launcher_open
            && !self.calendar_open
            && !self.quick_settings_open
            && !self.context_menu.open
            && self.dialog.is_none()
            && !self.desk_marquee_active
            && self.drag_win.is_none()
            && self.resize_win.is_none()
            && self.theme.surface_alpha >= 235
            && (self.toast_text.is_empty() || self.toast_ttl == 0)
            && !fps_overlap
            && band.intersect(self.dock_rect().inset(-18, -42)).is_empty();
        if no_overlays {
            if let Some((top_idx, _)) = self
                .windows
                .iter()
                .enumerate()
                .filter(|(_, w)| !w.minimized)
                .max_by_key(|(_, w)| w.z_order)
            {
                let wb = self.windows[top_idx].body_rect();
                if band.x >= wb.x && band.y >= wb.y && band.right() <= wb.right() && band.bottom() <= wb.bottom() {
                    let mut cv = Canvas::new(pixels, self.width, self.height, row_px);
                    cv.clip = band;
                    self.windows[top_idx].ensure_cache(
                        self.mouse_x,
                        self.mouse_y,
                        &self.theme,
                        &self.cfg,
                        &self.fonts,
                        &self.telemetry,
                        &self.bench,
                    );
                    self.windows[top_idx].blit_cached_clipped(&mut cv, &self.theme, false);
                    cv.apply_brightness(band, &self.theme);
                    return;
                }
            }
        }

        if self.locked {
            blit_wallpaper_rect(pixels, &self.wallpaper_cache, row_px, band);
            let mut cv = Canvas::new(pixels, self.width, self.height, row_px);
            cv.clip = band;
            self.draw_lock_screen(&mut cv);
            cv.apply_brightness(band, &self.theme);
            return;
        }

        // Open windows sorted by z-order (back-to-front)
        let drag_id = self.drag_win.map(|(id, _, _)| id);
        let mut order: Vec<usize> = self.windows.iter().enumerate().filter(|(_, w)| !w.minimized).map(|(i, _)| i).collect();
        order.sort_by_key(|&i| self.windows[i].z_order);

        let use_occlusion = self.theme.surface_alpha >= 235;
        if use_occlusion {
            // Front-to-back exact occlusion culling: O(1) pixel overdraw regardless of how many windows are open!
            let mut uncovered: Vec<Rect> = vec![band];
            let mut visible_layers: Vec<(usize, Vec<Rect>)> = Vec::with_capacity(order.len());
            for &idx in order.iter().rev() {
                if uncovered.is_empty() {
                    break;
                }
                let wr = self.windows[idx].rect;
                let padded = Rect::new(wr.x - 10, wr.y - 6, wr.w + 20, wr.h + 18);
                let mut vis: Vec<Rect> = Vec::new();
                for &u in &uncovered {
                    let inter = u.intersect(padded);
                    if !inter.is_empty() {
                        vis.push(inter);
                    }
                }
                if !vis.is_empty() {
                    visible_layers.push((idx, vis));
                    if uncovered.len() <= 24 {
                        subtract_rect(&mut uncovered, self.windows[idx].opaque_rect());
                    }
                }
            }

            // 1. Copy wallpaper strictly into uncovered background slivers (0 bytes copied when covered by windows!)
            for &ur in &uncovered {
                blit_wallpaper_rect(pixels, &self.wallpaper_cache, row_px, ur);
            }

            let mut cv = Canvas::new(pixels, self.width, self.height, row_px);

            // 2. Desktop shortcuts strictly where uncovered
            let icon_col = Rect::new(0, TOP_BAR_H, 120, 420);
            for &ur in &uncovered {
                let ir = ur.intersect(icon_col);
                if !ir.is_empty() {
                    cv.clip = ir;
                    self.draw_desktop_icons(&mut cv);
                }
            }
            cv.clip = band;
            if self.desk_marquee_active {
                let mr = Rect::from_points(self.desk_marquee_start.0, self.desk_marquee_start.1, self.desk_marquee_cur.0, self.desk_marquee_cur.1);
                if mr.w >= 2 || mr.h >= 2 {
                    let draw_r = Rect::new(mr.x, mr.y, mr.w.max(2), mr.h.max(2));
                    cv.fill_rounded_rect(draw_r, 4, Color::rgba(0x3B, 0x82, 0xF6, 0x44));
                    cv.stroke_rounded_rect(draw_r, 4, Color::rgb(0x60, 0xA5, 0xFA));
                }
            }

            // 3. Top bar
            if band.y < TOP_BAR_H {
                self.draw_top_bar(&mut cv);
            }

            // 4. Composite ONLY visible window sub-rects (back-to-front); hidden windows do 0 work!
            for (idx, vis) in visible_layers.into_iter().rev() {
                self.windows[idx].ensure_cache(
                    self.mouse_x,
                    self.mouse_y,
                    &self.theme,
                    &self.cfg,
                    &self.fonts,
                    &self.telemetry,
                    &self.bench,
                );
                for vr in vis {
                    cv.clip = vr;
                    self.windows[idx].blit_cached_clipped(&mut cv, &self.theme, true);
                }
            }
            cv.clip = band;

            // 5. Overlays, HUD, brightness & power transition
            self.draw_post_window_overlays(&mut cv, band);
            return;
        }

        // Translucent frosted-glass fallback (when user lowered window opacity < 92%)
        blit_wallpaper_rect(pixels, &self.wallpaper_cache, row_px, band);
        let mut cv = Canvas::new(pixels, self.width, self.height, row_px);
        cv.clip = band;
        if !Rect::new(0, TOP_BAR_H, 120, 420).intersect(band).is_empty() {
            self.draw_desktop_icons(&mut cv);
        }
        if self.desk_marquee_active {
            let mr = Rect::from_points(self.desk_marquee_start.0, self.desk_marquee_start.1, self.desk_marquee_cur.0, self.desk_marquee_cur.1);
            if mr.w >= 2 || mr.h >= 2 {
                let draw_r = Rect::new(mr.x, mr.y, mr.w.max(2), mr.h.max(2));
                cv.fill_rounded_rect(draw_r, 4, Color::rgba(0x3B, 0x82, 0xF6, 0x44));
                cv.stroke_rounded_rect(draw_r, 4, Color::rgb(0x60, 0xA5, 0xFA));
            }
        }
        if band.y < TOP_BAR_H {
            self.draw_top_bar(&mut cv);
        }
        for idx in order {
            let wr = self.windows[idx].rect;
            let padded = Rect::new(wr.x - 10, wr.y - 6, wr.w + 20, wr.h + 18);
            if !padded.intersect(band).is_empty() {
                let dragging_this = drag_id == Some(self.windows[idx].id);
                self.windows[idx].draw_cached(
                    &mut cv,
                    self.mouse_x,
                    self.mouse_y,
                    &self.theme,
                    &self.cfg,
                    &self.fonts,
                    &self.telemetry,
                    &self.bench,
                    dragging_this,
                );
            }
        }
        self.draw_post_window_overlays(&mut cv, band);
    }

    fn draw_post_window_overlays(&self, cv: &mut Canvas<'_>, band: Rect) {
        // 5. Overlays: Launcher, Calendar, Quick Settings, Dock
        if self.launcher_open { self.draw_launcher(cv); }
        if self.calendar_open { self.draw_calendar_popover(cv); }
        if self.quick_settings_open { self.draw_quick_settings(cv); }
        if !self.dock_rect().inset(-18, -42).intersect(band).is_empty() {
            self.draw_dock(cv);
        }

        // 6. Floating context menu, system dialogs, toast
        let pal = self.theme.palette();
        if self.context_menu.open {
            self.context_menu.draw(cv, self.mouse_x, self.mouse_y, &pal, self.theme.blur_enabled, &self.fonts.sm, &self.fonts.md);
        }
        if self.dialog.is_some() {
            self.draw_active_dialog(cv);
        }
        if !self.toast_text.is_empty() && self.toast_ttl > 0 && band.y <= TOP_BAR_H + 64 {
            self.draw_toast(cv);
        }

        // 7. Optional Developer FPS / GPU / render-time HUD (Spec §1.3 & §1.19)
        if (self.theme.show_fps || self.cfg.dev_mode) && !band.intersect(self.fps_hud_rect()).is_empty() {
            let gpu_tag = if self.gpu_hw_active { "VirtIO-GPU 2D" } else { "VBE-2D" };
            let txt = format!(
                "{} Hz • {}.{} ms • {}",
                self.measured_fps.clamp(1, 144),
                self.last_render_us / 1000,
                (self.last_render_us % 1000) / 100,
                gpu_tag
            );
            let tw = self.fonts.sm.measure_width(&txt) + 18;
            let r = Rect::new(self.width as i32 - tw - 12, TOP_BAR_H + 10, tw, 24);
            cv.fill_rounded_rect(r, 7, pal.menu_bg);
            cv.stroke_rounded_rect(r, 7, pal.border_subtle);
            cv.draw_text(&self.fonts.sm, r.x + 9, r.y + 4, &txt, pal.accent);
        }

        // 8. Global live brightness & Night Light calibration
        cv.apply_brightness(band, &self.theme);

        // 9. Smooth 60 Hz Shutdown / Reboot full-screen transition overlay
        if self.power_transition.is_some() {
            self.draw_power_transition(cv);
        }
    }

    fn draw_desktop_icons(&self, cv: &mut Canvas<'_>) {
        let pal = self.theme.palette();
        let light_wp = self.theme.wallpaper_idx == 1;
        for (i, item) in DESKTOP_SHORTCUTS.iter().enumerate() {
            let r = Self::desktop_icon_rect(i);
            let sel = self.desk_selected.contains(&i);
            let hov = r.contains(self.mouse_x, self.mouse_y) && self.windows.iter().all(|w| w.minimized || !w.rect.contains(self.mouse_x, self.mouse_y));
            if sel {
                cv.fill_rounded_rect(r, 11, pal.accent.with_alpha(0x32));
                cv.stroke_rounded_rect(r, 11, pal.accent);
            } else if hov {
                cv.fill_rounded_rect(r, 11, Color::rgba(0xFF, 0xFF, 0xFF, 0x1E));
            }
            cv.blit_rgba(r.x + 21, r.y + 8, 42, 42, item.icon.rgba_40(), 40, 40, 255);
            let lbl = if self.cfg.lang.is_en() { item.label_en } else { item.label_fr };
            let lw = self.fonts.sm.measure_width(lbl);
            let lx = r.x + (r.w - lw) / 2;
            let ly = r.y + 56;
            if !light_wp {
                cv.draw_text(&self.fonts.sm, lx + 1, ly + 1, lbl, Color::rgba(0, 0, 0, 0xB8));
            }
            cv.draw_text(&self.fonts.sm, lx, ly, lbl, if light_wp { Color::rgb(0x18, 0x1B, 0x24) } else { Color::WHITE });
        }
    }

    fn draw_toast(&self, cv: &mut Canvas<'_>) {
        let pal = self.theme.palette();
        let tw = self.fonts.md.measure_width(&self.toast_text) + 32;
        let w = tw.clamp(200, self.width as i32 - 48);
        let r = Rect::new((self.width as i32 - w) / 2, TOP_BAR_H + 14, w, 34);
        cv.draw_panel(r, 11, pal.menu_bg, pal.accent, false, true);
        cv.draw_text_ellipsis(&self.fonts.md, r.x + 16, r.y + 8, &self.toast_text, r.w - 32, pal.text_primary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::AppAction;
    use exo_syscall_abi as syscall;
    use exo_ui::ThemeMode;

    fn key_press(code: u16, ascii: u8, mods: u8) -> syscall::InputEventWire {
        syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_KEYBOARD,
            state: syscall::INPUT_KEY_PRESSED,
            code,
            value: 1,
            ascii,
            modifiers: mods,
            _pad: [0; 4],
        }
    }

    #[test]
    fn boot_starts_with_zero_windows_and_no_kbd_grab() {
        let desk = DesktopShell::new(1280, 720);
        assert!(desk.windows.is_empty());
        assert!(!desk.launcher_open);
        assert!(!desk.needs_kbd_grab());
        assert_eq!(desk.cfg.lang, crate::i18n::Lang::En);
        assert_eq!(desk.cfg.username, "user");
    }

    #[test]
    fn dark_light_bilingual_bench_and_shortcuts() {
        let mut desk = DesktopShell::new(1280, 720);
        let mut fb = vec![0xFF00_0000u32; 1280 * 720];

        // Super toggles launcher
        desk.on_input_event(key_press(0x00e3, 0, syscall::INPUT_MOD_META));
        assert!(desk.launcher_open);
        assert!(desk.needs_kbd_grab());
        desk.render(&mut fb);

        // Run all-module ExoBench & render in Light Mode + English
        desk.process_action(AppAction::RunBenchAll, None);
        assert_eq!(desk.bench.reports.len(), 5);
        desk.theme.mode = ThemeMode::Light;
        desk.theme.wallpaper_idx = 1;
        desk.cfg.lang = crate::i18n::Lang::En;
        desk.open_or_focus_app(AppKind::Bench);
        desk.open_or_focus_app(AppKind::Settings);
        desk.open_or_focus_app(AppKind::Games);
        desk.render(&mut fb);
        assert_ne!(fb[360 * 1280 + 640], 0xFF00_0000);
    }

    #[test]
    fn mouse_marquee_and_editor_drag_selection_work() {
        let mut desk = DesktopShell::new(1280, 720);
        let mut fb = vec![0xFF00_0000u32; 1280 * 720];

        let mouse_ev = |code: u16, value: i16| syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_MOUSE,
            state: if value == 0 { syscall::INPUT_KEY_RELEASED } else { syscall::INPUT_KEY_PRESSED },
            code,
            value,
            ascii: 0,
            modifiers: 0,
            _pad: [0; 4],
        };

        // 1. Desktop blue marquee selection drag
        desk.mouse_x = 300;
        desk.mouse_y = 200;
        desk.on_input_event(mouse_ev(0x0100, 1));
        assert!(desk.desk_marquee_active);
        desk.on_input_event(mouse_ev(0x0110, 140));
        desk.on_input_event(mouse_ev(0x0111, 90));
        desk.render(&mut fb);
        // Border top edge of marquee at (350, 200) must be crisp blue #60A5FA
        assert_eq!(fb[200 * 1280 + 350] & 0x00FF_FFFF, 0x0060_A5FA);
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(!desk.desk_marquee_active);

        // 2. Text Editor mouse drag text selection + Ctrl+C
        let wid = desk.open_or_focus_app(AppKind::Editor);
        let body = desk.windows.iter().find(|w| w.id == wid).unwrap().body_rect();
        desk.mouse_x = body.x + 60;
        desk.mouse_y = body.y + 54; // Line 1 inside editor area
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0110, 120));
        desk.on_input_event(mouse_ev(0x0100, 0));
        let has_sel = match &desk.windows.iter().find(|w| w.id == wid).unwrap().app {
            crate::desktop::window::AppInstance::Editor(ed) => ed.editor.has_selection() && !ed.editor.selected_text().is_empty(),
            _ => false,
        };
        assert!(has_sel, "mouse drag in Editor must select text");
    }

    #[test]
    fn window_drag_right_click_and_settings_options_work() {
        let mut desk = DesktopShell::new(1280, 720);
        let mouse_ev = |code: u16, value: i16| syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_MOUSE,
            state: if value == 0 { syscall::INPUT_KEY_RELEASED } else { syscall::INPUT_KEY_PRESSED },
            code,
            value,
            ascii: 0,
            modifiers: 0,
            _pad: [0; 4],
        };

        // 1. Normal window drag via titlebar
        let fid = desk.open_or_focus_app(AppKind::Files);
        let orig = desk.windows.iter().find(|w| w.id == fid).unwrap().rect;
        desk.mouse_x = orig.x + 120;
        desk.mouse_y = orig.y + 18;
        desk.on_input_event(mouse_ev(0x0100, 1));
        assert!(desk.drag_win.is_some(), "pressing titlebar must start drag");
        desk.on_input_event(mouse_ev(0x0110, 100));
        desk.on_input_event(mouse_ev(0x0111, 50));
        desk.on_input_event(mouse_ev(0x0100, 0));
        let moved = desk.windows.iter().find(|w| w.id == fid).unwrap().rect;
        assert!(moved.x > orig.x + 50 && moved.y > orig.y + 20, "window must move during drag: orig={:?}, moved={:?}", orig, moved);

        // 2. Dragging a maximized window restores & moves it
        let work = desk.work_area();
        desk.windows.iter_mut().find(|w| w.id == fid).unwrap().toggle_maximize(work);
        assert!(desk.windows.iter().find(|w| w.id == fid).unwrap().maximized);
        desk.mouse_x = 400;
        desk.mouse_y = work.y + 16;
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0110, 40));
        desk.on_input_event(mouse_ev(0x0111, 30));
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(!desk.windows.iter().find(|w| w.id == fid).unwrap().maximized, "dragging maximized window must restore it");

        // 3. Right-click inside Files -> "Ouvrir Terminal ici" spawns a Terminal window
        let frect = desk.windows.iter().find(|w| w.id == fid).unwrap().body_rect();
        desk.mouse_x = frect.x + 280;
        desk.mouse_y = frect.y + 220;
        desk.on_input_event(mouse_ev(0x0101, 1));
        desk.on_input_event(mouse_ev(0x0101, 0));
        assert!(desk.context_menu.open, "right-click in Files must open context menu");
        let term_idx = desk.context_menu.items.iter().position(|it| it.id == crate::apps::ctx_cmd::FILES_OPEN_TERM)
            .expect("Files context menu must contain 'Ouvrir Terminal ici'");
        let mb = desk.context_menu.bounds();
        desk.mouse_x = mb.x + 40;
        desk.mouse_y = mb.y + 6 + (term_idx as i32) * 30 + 15;
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(!desk.context_menu.open);
        assert!(desk.windows.iter().any(|w| w.kind == AppKind::Terminal), "clicking 'Ouvrir Terminal ici' must open Terminal");

        // 4. Settings Display Night Light + FPS toggle + PowerDev Developer Mode toggle
        let sid = desk.open_or_focus_app(AppKind::Settings);
        desk.process_action(AppAction::OpenSettingsCategory(1), Some(sid)); // Display
        let sbody = desk.windows.iter().find(|w| w.id == sid).unwrap().body_rect();
        let (_, scontent) = crate::apps::settings::SettingsApp::layout(sbody);
        // Click Night Light card (whole row clickable: y = 322..374)
        desk.mouse_x = scontent.x + 120;
        desk.mouse_y = scontent.y + 344;
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(desk.theme.night_light, "clicking Night Light row must enable night_light");
        // Click FPS card (whole row clickable: y = 386..440)
        desk.mouse_x = scontent.x + 120;
        desk.mouse_y = scontent.y + 410;
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(desk.theme.show_fps, "clicking FPS row must enable show_fps");
        // Switch to Power & Dev (category 8) and click Developer Mode row (y = 266..326)
        desk.process_action(AppAction::OpenSettingsCategory(8), Some(sid));
        desk.mouse_x = scontent.x + 120;
        desk.mouse_y = scontent.y + 290;
        desk.on_input_event(mouse_ev(0x0100, 1));
        desk.on_input_event(mouse_ev(0x0100, 0));
        assert!(desk.cfg.dev_mode, "clicking Developer Mode row must toggle dev_mode");

        // 5. Files opens outside `/` & Editor saves cleanly without `/` permission failure
        let f_cwd = match &desk.windows.iter().find(|w| w.id == fid).unwrap().app {
            crate::desktop::window::AppInstance::Files(f) => f.cwd.clone(),
            _ => String::new(),
        };
        assert_ne!(f_cwd, "/", "Files must open in a writable user folder outside /");

        let eid = desk.open_or_focus_app(AppKind::Editor);
        if let Some(w) = desk.windows.iter_mut().find(|w| w.id == eid) {
            if let crate::desktop::window::AppInstance::Editor(ref mut ed) = w.app {
                ed.path_field = exo_ui::TextField::with_text("/blocked_root.txt");
                ed.save();
                assert!(
                    ed.status.starts_with("Saved: ") || ed.status.starts_with("Enregistré : "),
                    "Editor must redirect & save cleanly: {}",
                    ed.status
                );
                assert_ne!(ed.file_path, "/blocked_root.txt");
            }
        }

        // 6. Network diagnostic & ping parser work cleanly
        assert_eq!(crate::net_diag::parse_ipv4("1.1.1.1"), Some([1, 1, 1, 1]));
        let ping_lines = crate::net_diag::run_ping_command("127.0.0.1", 1);
        assert!(ping_lines.iter().any(|(l, _)| l.contains("127.0.0.1")));
    }

    #[test]
    fn universal_scrolling_sustained_paste_and_power_animation_work() {
        let mut desk = DesktopShell::new(1280, 720);
        let mut fb = vec![0xFF00_0000u32; 1280 * 720];
        let wheel_ev = |dz: i16| syscall::InputEventWire {
            device: syscall::INPUT_DEVICE_MOUSE,
            state: syscall::INPUT_KEY_PRESSED,
            code: 0x0112,
            value: dz,
            ascii: 0,
            modifiers: 0,
            _pad: [0; 4],
        };

        // 1. Editor: sustained Ctrl+V paste (50x) stays capped, scopes damage to window rect, and scrolls via wheel!
        let eid = desk.open_or_focus_app(AppKind::Editor);
        let _ = desk.take_dirty_rect();
        desk.clipboard.set_text("Ligne de test ExoOS pour vérifier le défilement fluide 60Hz\n");
        for _ in 0..50 {
            desk.on_input_event(key_press(0x0019, b'v', syscall::INPUT_MOD_CTRL));
        }
        let dmg = desk.take_dirty_rect().expect("paste must produce damage");
        assert!(dmg.w < 1280 || dmg.h < 720, "Editor typing/paste damage must be window-scoped, got {:?}", dmg);
        let er = desk.windows.iter().find(|w| w.id == eid).unwrap().rect;
        desk.mouse_x = er.x + 180;
        desk.mouse_y = er.y + 180;
        let scroll_before = match &desk.windows.iter().find(|w| w.id == eid).unwrap().app {
            crate::desktop::window::AppInstance::Editor(ed) => ed.scroll_y,
            _ => 0,
        };
        assert!(scroll_before > 0, "Auto-scroll must track cursor down after pasting 50 lines");
        desk.on_input_event(wheel_ev(-3));
        let scroll_after_up = match &desk.windows.iter().find(|w| w.id == eid).unwrap().app {
            crate::desktop::window::AppInstance::Editor(ed) => ed.scroll_y,
            _ => 0,
        };
        assert!(scroll_after_up < scroll_before, "Wheel up (-3) must scroll Editor upward");
        desk.render(&mut fb);

        // 2. Settings: wheel scrolls down and back up
        let sid = desk.open_or_focus_app(AppKind::Settings);
        let sr = desk.windows.iter().find(|w| w.id == sid).unwrap().rect;
        desk.mouse_x = sr.x + 300;
        desk.mouse_y = sr.y + 200;
        desk.on_input_event(wheel_ev(3));
        let s_scroll = match &desk.windows.iter().find(|w| w.id == sid).unwrap().app {
            crate::desktop::window::AppInstance::Settings(s) => s.scroll_y,
            _ => 0,
        };
        assert!(s_scroll > 0, "Wheel down (+3) must scroll Settings content");
        desk.render(&mut fb);

        // 3. Terminal: wheel scrolls up into output history & PageUp/PageDown work
        let tid = desk.open_or_focus_app(AppKind::Terminal);
        if let Some(w) = desk.windows.iter_mut().find(|w| w.id == tid) {
            if let crate::desktop::window::AppInstance::Terminal(ref mut term) = w.app {
                for i in 0..40 {
                    term.cur_mut().push_line(&format!("exo_line_{}", i), Color::WHITE);
                }
            }
        }
        let tr = desk.windows.iter().find(|w| w.id == tid).unwrap().rect;
        desk.mouse_x = tr.x + 200;
        desk.mouse_y = tr.y + 180;
        desk.on_input_event(wheel_ev(-3));
        let t_off = match &desk.windows.iter().find(|w| w.id == tid).unwrap().app {
            crate::desktop::window::AppInstance::Terminal(t) => t.cur().scroll_offset,
            _ => 0,
        };
        assert!(t_off > 0, "Wheel up (-3) must scroll Terminal into history");
        desk.render(&mut fb);

        // 4. Shutdown animation progresses 0 -> 0.5 -> 1.0 and only triggers power_request after final frame!
        desk.process_action(AppAction::Shutdown, None);
        assert!(desk.is_power_transitioning());
        assert!(desk.take_power_request().is_none(), "Power action must wait for animation");
        let t0 = desk.power_transition.unwrap().started_ns;
        desk.advance_power_transition(t0 + crate::desktop::state::POWER_ANIM_NS / 2);
        desk.render(&mut fb);
        assert_ne!(fb[360 * 1280 + 640], 0xFF00_0000); // Emblem/ring visible mid-animation
        desk.advance_power_transition(t0 + crate::desktop::state::POWER_ANIM_NS);
        desk.render(&mut fb);
        assert_eq!(fb[10 * 1280 + 10] & 0x00FF_FFFF, 0x0000_0000); // Final frame faded to pure black
        desk.on_frame_presented();
        assert_eq!(desk.take_power_request(), Some(crate::desktop::PowerRequest::Shutdown));
    }

    #[test]
    fn occlusion_culling_multi_rect_damage_and_six_window_60hz_budget() {
        let mut desk = DesktopShell::new(1920, 1080);
        let mut shm = vec![0xFF00_0000u32; 1920 * 1080];

        // Open 6 simultaneous apps just like a heavy real desktop session
        let fid = desk.open_or_focus_app(AppKind::Files);
        let gid = desk.open_or_focus_app(AppKind::Games);
        let eid = desk.open_or_focus_app(AppKind::Editor);
        let mid = desk.open_or_focus_app(AppKind::Monitor);
        let bid = desk.open_or_focus_app(AppKind::Bench);
        let sid = desk.open_or_focus_app(AppKind::Settings);
        desk.process_action(AppAction::RunBenchAll, Some(bid));
        desk.process_action(AppAction::OpenSettingsCategory(1), Some(sid)); // Display

        // Place Games completely buried underneath Settings (to verify 100% occlusion skip)
        let s_rect = desk.windows.iter().find(|w| w.id == sid).unwrap().rect;
        if let Some(g) = desk.windows.iter_mut().find(|w| w.id == gid) {
            g.rect = Rect::new(s_rect.x + 20, s_rect.y + 40, 500, 400);
        }
        // Place Monitor at bottom-left, ExoBench at bottom-right, Editor at top-left, Files top-center
        if let Some(m) = desk.windows.iter_mut().find(|w| w.id == mid) {
            m.rect = Rect::new(20, 380, 820, 560);
        }
        if let Some(b) = desk.windows.iter_mut().find(|w| w.id == bid) {
            b.rect = Rect::new(1020, 360, 860, 560);
        }
        if let Some(e) = desk.windows.iter_mut().find(|w| w.id == eid) {
            e.rect = Rect::new(50, 80, 740, 480);
        }
        if let Some(f) = desk.windows.iter_mut().find(|w| w.id == fid) {
            f.rect = Rect::new(420, 60, 780, 480);
        }

        // Initial warm-up render
        desk.dirty = true;
        for r in desk.take_dirty_rects() {
            desk.render_rect_to_shm(&mut shm, r);
        }

        // 1. Buried Games window must have ZERO visible unoccluded rects!
        let g_rect = desk.windows.iter().find(|w| w.id == gid).unwrap().body_rect();
        assert!(desk.visible_window_rects(gid, g_rect).is_empty(), "buried window must be 100% occluded");

        // 2. Clicking inside ALREADY-focused Settings must NOT trigger a full-screen repaint!
        desk.focus_window(sid);
        assert!(desk.take_dirty_rects().is_empty(), "refocusing already topmost window must be a no-op");

        // 3. Periodic telemetry tick with 6 windows open must NOT invalidate Settings(Display) or ExoBench!
        desk.tick_periodic();
        assert!(!desk.windows.iter().find(|w| w.id == sid).unwrap().cache_dirty, "Settings Display must stay cached");
        assert!(!desk.windows.iter().find(|w| w.id == bid).unwrap().cache_dirty, "Static ExoBench must stay cached");
        let periodic_rects = desk.take_dirty_rects();
        assert!(!periodic_rects.is_empty(), "Partially visible Monitor must emit tight dirty rects");
        let total_px: i64 = periodic_rects.iter().map(|r| r.w as i64 * r.h as i64).sum();
        assert!(total_px < 1920 * 1080 / 2, "Periodic 6-window dirty area must stay small (got {} px)", total_px);

        // 4. Compositing those dirty rects must keep Settings cache clean & render cleanly!
        for r in periodic_rects {
            desk.render_rect_to_shm(&mut shm, r);
        }
        assert!(!desk.windows.iter().find(|w| w.id == sid).unwrap().cache_dirty);
        assert_ne!(shm[500 * 1920 + 900], 0xFF00_0000);
    }

    #[test]
    #[ignore]
    fn export_visual_previews() {
        let dir = std::env::var("EXO_PREVIEW_DIR").unwrap_or_else(|_| String::from("/tmp"));
        let (w, h) = (1280u32, 720u32);
        let mut fb = vec![0xFF00_0000u32; (w * h) as usize];

        let save = |name: &str, px: &[u32]| {
            let mut out = Vec::with_capacity((w * h * 3) as usize + 32);
            out.extend_from_slice(format!("P6\n{} {}\n255\n", w, h).as_bytes());
            for &p in px {
                out.push(((p >> 16) & 0xFF) as u8);
                out.push(((p >> 8) & 0xFF) as u8);
                out.push((p & 0xFF) as u8);
            }
            std::fs::write(format!("{}/{}", dir, name), &out).expect("write ppm");
        };

        // 1. Clean Dark Desktop + Launcher + Multi-color centered dock
        let mut d1 = DesktopShell::new(w, h);
        d1.launcher_open = true;
        d1.render(&mut fb);
        save("v2_01_dark_desktop_launcher.ppm", &fb);

        // 2. Light Mode + Personalization Settings + Calendar Popover
        let mut d2 = DesktopShell::new(w, h);
        d2.theme.mode = ThemeMode::Light;
        d2.theme.wallpaper_idx = 1;
        d2.open_or_focus_app(AppKind::Settings);
        d2.quick_settings_open = true;
        d2.render(&mut fb);
        save("v2_02_light_settings_quick.ppm", &fb);

        // 3. ExoBench All-Module Performance Profiler & Report
        let mut d3 = DesktopShell::new(w, h);
        d3.process_action(AppAction::RunBenchAll, None);
        d3.open_or_focus_app(AppKind::Bench);
        d3.render(&mut fb);
        save("v2_03_exobench_report.ppm", &fb);

        // 4. Files + Trash + Capability Picker / Games side-by-side
        let mut d4 = DesktopShell::new(w, h);
        d4.open_or_focus_app(AppKind::Games);
        d4.windows[0].rect = Rect::new(660, 86, 580, 500);
        d4.open_or_focus_app(AppKind::Files);
        d4.windows[1].rect = Rect::new(112, 48, 820, 540);
        d4.render(&mut fb);
        save("v2_04_files_games.ppm", &fb);
    }
}
