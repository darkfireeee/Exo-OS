//! Clean flagship OS top bar, Calendar popover (§3.4), and Quick Settings popover (§3.6).

use crate::apps::AppAction;
use crate::assets::ICON_START_40;
use crate::desktop::state::{DesktopShell, TOP_BAR_H};
use exo_ui::widgets::{draw_button, draw_slider, draw_toggle, slider_ratio_from_x};
use exo_ui::{Canvas, Color, Rect, ThemeMode};

impl DesktopShell {
    pub(crate) fn topbar_clock_rect(&self) -> Rect {
        let w = 216;
        Rect::new((self.width as i32 - w) / 2, 4, w, 26)
    }

    pub(crate) fn topbar_keymap_rect(&self) -> Rect {
        Rect::new(self.width as i32 - 216, 5, 42, 24)
    }

    pub(crate) fn topbar_theme_rect(&self) -> Rect {
        Rect::new(self.width as i32 - 168, 5, 68, 24)
    }

    pub(crate) fn topbar_quick_rect(&self) -> Rect {
        Rect::new(self.width as i32 - 94, 5, 84, 24)
    }

    pub(crate) fn calendar_popover_rect(&self) -> Rect {
        let w = 276;
        Rect::new((self.width as i32 - w) / 2, TOP_BAR_H + 8, w, 244)
    }

    pub(crate) fn quick_settings_rect(&self) -> Rect {
        Rect::new(self.width as i32 - 308, TOP_BAR_H + 8, 296, 266)
    }

    pub(crate) fn draw_top_bar(&self, cv: &mut Canvas<'_>) {
        let pal = self.theme.palette();
        let lang = self.cfg.lang;
        cv.fill_rect(Rect::new(0, 0, self.width as i32, TOP_BAR_H), pal.topbar_bg);
        cv.fill_rect(Rect::new(0, TOP_BAR_H - 1, self.width as i32, 1), pal.border_subtle);

        // Left: Minimal ExoOS identity + active window title
        cv.blit_rgba(12, 6, 22, 22, ICON_START_40, 40, 40, 255);
        cv.draw_text(&self.fonts.md, 40, 8, "ExoOS", pal.text_primary);
        if let Some(wid) = self.focused_window_id() {
            if let Some(w) = self.windows.iter().find(|w| w.id == wid) {
                cv.draw_text(&self.fonts.sm, 102, 10, "•", pal.text_muted);
                cv.draw_text_ellipsis(&self.fonts.sm, 116, 9, &w.title, 260, pal.text_secondary);
            }
        }

        // Center: Clickable Date & Clock pill
        let cr = self.topbar_clock_rect();
        if self.calendar_open || cr.contains(self.mouse_x, self.mouse_y) {
            cv.fill_rounded_rect(cr, 8, pal.hover_bg);
        }
        let clock_str = format!(
            "{}   {}",
            self.telemetry.clock.format_date(lang),
            self.telemetry.clock.format_time(self.cfg.use_24h)
        );
        let cw = self.fonts.sm.measure_width(&clock_str);
        cv.draw_text(&self.fonts.sm, cr.x + (cr.w - cw) / 2, cr.y + 5, &clock_str, pal.text_primary);

        // Right: Active keyboard layout (FR/US/BP), Theme toggle (Clair/Sombre), Quick Settings pill
        let kr = self.topbar_keymap_rect();
        if kr.contains(self.mouse_x, self.mouse_y) {
            cv.fill_rounded_rect(kr, 7, pal.hover_bg);
        }
        let ks = self.cfg.keymap.short();
        let kw = self.fonts.sm.measure_width(ks);
        cv.draw_text(&self.fonts.sm, kr.x + (kr.w - kw) / 2, kr.y + 4, ks, pal.text_secondary);

        let tr = self.topbar_theme_rect();
        if tr.contains(self.mouse_x, self.mouse_y) {
            cv.fill_rounded_rect(tr, 7, pal.hover_bg);
        }
        let tlbl = match self.theme.mode {
            ThemeMode::Dark => lang.pick("Sombre", "Dark"),
            ThemeMode::Light => lang.pick("Clair", "Light"),
        };
        let tw = self.fonts.sm.measure_width(tlbl);
        cv.draw_text(&self.fonts.sm, tr.x + (tr.w - tw) / 2, tr.y + 4, tlbl, pal.text_secondary);

        let qr = self.topbar_quick_rect();
        cv.fill_rounded_rect(qr, 8, if self.quick_settings_open { pal.accent.with_alpha(0x28) } else if qr.contains(self.mouse_x, self.mouse_y) { pal.hover_bg } else { Color::TRANSPARENT });
        let qtxt = format!("{}%", self.theme.brightness_pct);
        cv.fill_circle_aa(qr.x as f32 + 12.0, qr.y as f32 + 12.0, 4.0, pal.success);
        cv.draw_text(&self.fonts.sm, qr.x + 22, qr.y + 4, &qtxt, pal.text_primary);
    }

    pub(crate) fn draw_calendar_popover(&self, cv: &mut Canvas<'_>) {
        if !self.calendar_open { return; }
        let pal = self.theme.palette();
        let r = self.calendar_popover_rect();
        cv.draw_panel(r, 14, pal.menu_bg, pal.border_strong, self.theme.blur_enabled, true);

        let title = self.telemetry.clock.month_title(self.cfg.lang);
        let tw = self.fonts.md.measure_width(&title);
        cv.draw_text(&self.fonts.md, r.x + (r.w - tw) / 2, r.y + 12, &title, pal.text_primary);

        let headers = if self.cfg.lang.is_en() {
            ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
        } else {
            ["Lu", "Ma", "Me", "Je", "Ve", "Sa", "Di"]
        };
        let cw = (r.w - 24) / 7;
        for (i, h) in headers.iter().enumerate() {
            cv.draw_text(&self.fonts.sm, r.x + 14 + (i as i32) * cw, r.y + 42, h, pal.text_muted);
        }

        let (days, first_wd) = self.telemetry.clock.month_grid_info();
        for d in 1..=days {
            let slot = (first_wd + d - 1) as i32;
            let col = slot % 7;
            let row = slot / 7;
            let cell = Rect::new(r.x + 12 + col * cw, r.y + 66 + row * 28, cw - 2, 24);
            let is_today = d == self.telemetry.clock.day;
            if is_today {
                cv.fill_rounded_rect(cell, 7, pal.accent);
            }
            let s = format!("{}", d);
            let sw = self.fonts.sm.measure_width(&s);
            cv.draw_text(
                &self.fonts.sm,
                cell.x + (cell.w - sw) / 2,
                cell.y + 4,
                &s,
                if is_today { Color::WHITE } else { pal.text_primary },
            );
        }
    }

    pub(crate) fn draw_quick_settings(&self, cv: &mut Canvas<'_>) {
        if !self.quick_settings_open { return; }
        let pal = self.theme.palette();
        let lang = self.cfg.lang;
        let r = self.quick_settings_rect();
        cv.draw_panel(r, 14, pal.menu_bg, pal.border_strong, self.theme.blur_enabled, true);

        cv.draw_text(&self.fonts.md, r.x + 16, r.y + 14, lang.pick("Paramètres rapides", "Quick Settings"), pal.text_primary);

        // Brightness live slider
        cv.draw_text(&self.fonts.sm, r.x + 16, r.y + 46, &format!("{} ({}%)", lang.pick("Luminosité", "Brightness"), self.theme.brightness_pct), pal.text_secondary);
        let sr = Rect::new(r.x + 16, r.y + 68, r.w - 32, 22);
        let ratio = (self.theme.brightness_pct.saturating_sub(35) as f32) / 95.0;
        draw_slider(cv, sr, ratio, pal.accent, &pal, sr.contains(self.mouse_x, self.mouse_y));

        // 2x2 quick action tiles: Theme, Language, Keymap, DND
        let bw = (r.w - 40) / 2;
        let t1 = Rect::new(r.x + 16, r.y + 104, bw, 34);
        let t2 = Rect::new(t1.right() + 8, r.y + 104, bw, 34);
        let t3 = Rect::new(r.x + 16, r.y + 146, bw, 34);
        let t4 = Rect::new(t3.right() + 8, r.y + 146, bw, 34);

        draw_button(cv, t1, if self.theme.mode == ThemeMode::Dark { lang.pick("Mode Clair", "Light Mode") } else { lang.pick("Mode Sombre", "Dark Mode") }, false, t1.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, t2, &format!("{} : {}", lang.pick("Langue", "Lang"), lang.code()), false, t2.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, t3, &format!("{} : {}", lang.pick("Clavier", "Keymap"), self.cfg.keymap.short()), false, t3.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, t4, lang.pick("Ne pas déranger", "Do Not Disturb"), self.cfg.dnd_enabled, t4.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);

        // Footer row: Settings, Lock, Power
        let fy = r.bottom() - 48;
        cv.fill_rect(Rect::new(r.x + 12, fy - 8, r.w - 24, 1), pal.border_subtle);
        let s_btn = Rect::new(r.x + 16, fy, 92, 32);
        let l_btn = Rect::new(s_btn.right() + 6, fy, 84, 32);
        let p_btn = Rect::new(l_btn.right() + 6, fy, 76, 32);
        draw_button(cv, s_btn, lang.pick("Paramètres", "Settings"), false, s_btn.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, l_btn, lang.pick("Verrouiller", "Lock"), false, l_btn.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        draw_button(cv, p_btn, lang.pick("Éteindre", "Power"), true, p_btn.contains(self.mouse_x, self.mouse_y), &pal, &self.fonts.sm);
        let _ = draw_toggle;
    }

    pub(crate) fn click_top_bar(&mut self, mx: i32, my: i32) {
        if mx < 110 {
            self.launcher_open = !self.launcher_open;
            self.quick_settings_open = false;
            self.calendar_open = false;
        } else if self.topbar_clock_rect().contains(mx, my) {
            self.calendar_open = !self.calendar_open;
            self.quick_settings_open = false;
            self.launcher_open = false;
        } else if self.topbar_keymap_rect().contains(mx, my) {
            self.cfg.keymap = self.cfg.keymap.next();
            self.show_toast(format!("Clavier : {}", self.cfg.keymap.code()));
        } else if self.topbar_theme_rect().contains(mx, my) {
            self.theme.mode = if self.theme.mode == ThemeMode::Dark { ThemeMode::Light } else { ThemeMode::Dark };
        } else if self.topbar_quick_rect().contains(mx, my) {
            self.quick_settings_open = !self.quick_settings_open;
            self.calendar_open = false;
            self.launcher_open = false;
        }
        self.dirty = true;
    }

    pub(crate) fn click_quick_settings(&mut self, mx: i32, my: i32) -> bool {
        if !self.quick_settings_open { return false; }
        let r = self.quick_settings_rect();
        if !r.contains(mx, my) {
            self.quick_settings_open = false;
            self.dirty = true;
            return false;
        }
        let sr = Rect::new(r.x + 16, r.y + 68, r.w - 32, 22);
        if sr.inset(-4, -8).contains(mx, my) {
            self.theme.brightness_pct = (35.0 + slider_ratio_from_x(sr, mx) * 95.0) as u16;
            self.dirty = true;
            return true;
        }
        let bw = (r.w - 40) / 2;
        let t1 = Rect::new(r.x + 16, r.y + 104, bw, 34);
        let t2 = Rect::new(t1.right() + 8, r.y + 104, bw, 34);
        let t3 = Rect::new(r.x + 16, r.y + 146, bw, 34);
        let t4 = Rect::new(t3.right() + 8, r.y + 146, bw, 34);
        if t1.contains(mx, my) {
            self.theme.mode = if self.theme.mode == ThemeMode::Dark { ThemeMode::Light } else { ThemeMode::Dark };
        } else if t2.contains(mx, my) {
            self.cfg.lang = if self.cfg.lang.is_en() { crate::i18n::Lang::Fr } else { crate::i18n::Lang::En };
        } else if t3.contains(mx, my) {
            self.cfg.keymap = self.cfg.keymap.next();
        } else if t4.contains(mx, my) {
            self.cfg.dnd_enabled = !self.cfg.dnd_enabled;
        }
        let fy = r.bottom() - 48;
        if Rect::new(r.x + 16, fy, 92, 32).contains(mx, my) {
            self.quick_settings_open = false;
            self.process_action(AppAction::OpenApp(crate::apps::AppKind::Settings), None);
        } else if Rect::new(r.x + 114, fy, 84, 32).contains(mx, my) {
            self.process_action(AppAction::LockScreen, None);
        } else if Rect::new(r.x + 204, fy, 76, 32).contains(mx, my) {
            self.process_action(AppAction::Shutdown, None);
        }
        self.dirty = true;
        true
    }
}
