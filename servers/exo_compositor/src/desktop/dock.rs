//! Centered floating taskbar / dock with smooth magnification transitions and multi-color icons.

use crate::apps::AppKind;
use crate::assets::ICON_START_40;
use crate::desktop::state::{DesktopShell, DOCK_BOTTOM_MARGIN, DOCK_H};
use exo_ui::{Canvas, Color, Rect, TextField};

impl DesktopShell {
    pub(crate) fn dock_rect(&self) -> Rect {
        // Start (50) + sep (16) + 7 apps (7 * 54) + padding (26) = 470
        let w = 474.min(self.width as i32 - 24);
        let x = (self.width as i32 - w) / 2;
        let y = self.height as i32 - DOCK_H - DOCK_BOTTOM_MARGIN;
        Rect::new(x, y, w, DOCK_H)
    }

    pub(crate) fn dock_start_btn(&self) -> Rect {
        let d = self.dock_rect();
        Rect::new(d.x + 12, d.y + 7, 48, 48)
    }

    pub(crate) fn dock_app_rect(&self, idx: usize) -> Rect {
        let d = self.dock_rect();
        Rect::new(d.x + 74 + (idx as i32) * 54, d.y + 7, 48, 48)
    }

    pub(crate) fn draw_dock(&self, cv: &mut Canvas<'_>) {
        let pal = self.theme.palette();
        let dock = self.dock_rect();
        cv.draw_panel(dock, 20, pal.dock_bg, pal.border_strong, self.theme.blur_enabled, true);

        let mut tooltip: Option<(i32, &str)> = None;

        // 1. Start Button featuring user's hexagonal crystal logo
        let sb = self.dock_start_btn();
        let sb_hov = sb.contains(self.mouse_x, self.mouse_y);
        if self.launcher_open || sb_hov {
            cv.fill_rounded_rect(sb, 13, pal.accent.with_alpha(if self.launcher_open { 0x36 } else { 0x22 }));
            cv.stroke_rounded_rect(sb, 13, pal.accent.with_alpha(0xA8));
        }
        let s0 = (38.0 * self.dock_scales[0]) as i32;
        let off0 = (sb.w - s0) / 2;
        cv.blit_rgba(sb.x + off0, sb.y + off0, s0, s0, ICON_START_40, 40, 40, 255);
        if sb_hov {
            tooltip = Some((sb.x + sb.w / 2, self.cfg.lang.pick("Applications (Super)", "Applications (Super)")));
        }

        // Subtle divider
        cv.fill_rect(Rect::new(sb.right() + 7, dock.y + 15, 1, dock.h - 30), pal.border_subtle);

        // 2. Harmonized multi-color pinned apps with smooth interpolated hover lift
        for (i, &kind) in AppKind::DOCK_APPS.iter().enumerate() {
            let r = self.dock_app_rect(i);
            let hov = r.contains(self.mouse_x, self.mouse_y);
            let running = self.windows.iter().any(|w| w.kind == kind);
            let focused = self.windows.iter().any(|w| w.kind == kind && !w.minimized && w.focused);

            if focused || hov {
                cv.fill_rounded_rect(r, 12, if focused { pal.accent.with_alpha(0x26) } else { pal.hover_bg });
            }

            let sc = self.dock_scales[i + 1];
            let sz = (38.0 * sc) as i32;
            let lift = ((sc - 1.0) * 32.0) as i32;
            let ox = r.x + (r.w - sz) / 2;
            let oy = r.y + (r.h - sz) / 2 - lift - 1;
            cv.blit_rgba(ox, oy, sz, sz, kind.icon().rgba_40(), 40, 40, 255);

            if running {
                let pw = if focused { 16 } else { 6 };
                let col = if focused { pal.accent } else { pal.text_muted };
                cv.fill_rounded_rect(Rect::new(r.x + (r.w - pw) / 2, dock.bottom() - 6, pw, 3), 2, col);
            }
            if hov {
                tooltip = Some((r.x + r.w / 2, kind.short_label(self.cfg.lang)));
            }
        }

        // Clean tooltip (hidden while launcher is open)
        if !self.launcher_open && !self.locked {
            if let Some((cx, text)) = tooltip {
                let tw = self.fonts.sm.measure_width(text) + 20;
                let tr = Rect::new((cx - tw / 2).clamp(8, self.width as i32 - tw - 8), dock.y - 34, tw, 26);
                cv.draw_panel(tr, 8, pal.menu_bg, pal.border_strong, false, true);
                cv.draw_text(&self.fonts.sm, tr.x + 10, tr.y + 5, text, if pal.is_dark { Color::WHITE } else { pal.text_primary });
            }
        }
    }

    pub(crate) fn click_dock(&mut self, mx: i32, my: i32) -> bool {
        if !self.dock_rect().contains(mx, my) {
            return false;
        }
        if self.dock_start_btn().contains(mx, my) {
            self.launcher_open = !self.launcher_open;
            self.quick_settings_open = false;
            self.calendar_open = false;
            if self.launcher_open {
                self.launcher_search = TextField::new();
            }
            self.dirty = true;
            return true;
        }
        for (i, &kind) in AppKind::DOCK_APPS.iter().enumerate() {
            if self.dock_app_rect(i).contains(mx, my) {
                self.close_popovers();
                self.handle_dock_app_click(kind);
                return true;
            }
        }
        true
    }

    fn handle_dock_app_click(&mut self, kind: AppKind) {
        if let Some(pos) = self.windows.iter().position(|w| w.kind == kind) {
            let id = self.windows[pos].id;
            if self.windows[pos].minimized {
                self.windows[pos].minimized = false;
                self.focus_window(id);
            } else if self.windows[pos].focused {
                self.windows[pos].minimized = true;
                self.windows[pos].focused = false;
                self.dirty = true;
            } else {
                self.focus_window(id);
            }
        } else {
            self.spawn_app(kind, None);
        }
    }
}
