//! Processes & Capabilities list with real `SYS_KILL` termination control (Spec §4.1).

use crate::apps::monitor::MonitorApp;
use crate::apps::AppAction;
use crate::assets::FontSet;
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{compute_scrollbar, draw_button, draw_scrollbar, scroll_from_drag_y};
use exo_ui::{Canvas, Palette, Rect};

impl MonitorApp {
    pub(crate) fn draw_processes(&self, cv: &mut Canvas<'_>, c: Rect, mx: i32, my: i32, pal: &Palette, lang: Lang, fonts: &FontSet, t: &TelemetryState) {
        let pad = 16;
        let gap = 12;
        let left_w = ((c.w - pad * 2 - gap) * 62) / 100;
        let right_w = c.w - pad * 2 - gap - left_w;

        let pcard = Rect::new(c.x + pad, c.y + pad, left_w, c.h - pad * 2);
        cv.fill_rounded_rect(pcard, 12, pal.card_bg);
        cv.stroke_rounded_rect(pcard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, pcard.x + 14, pcard.y + 12, &format!("{} ({})", lang.pick("Processus actifs", "Active processes"), t.processes.len()), pal.text_primary);

        if let Some(pid) = self.selected_proc_pid {
            let kill_btn = Rect::new(pcard.right() - 148, pcard.y + 8, 134, 26);
            draw_button(cv, kill_btn, &format!("{} #{}", lang.pick("Arrêter PID", "Stop PID"), pid), true, kill_btn.contains(mx, my), pal, &fonts.sm);
        }

        let hy = pcard.y + 40;
        cv.fill_rect(Rect::new(pcard.x + 1, hy, pcard.w - 2, 24), pal.sidebar_bg);
        cv.draw_text(&fonts.sm, pcard.x + 14, hy + 4, "PID", pal.text_muted);
        cv.draw_text(&fonts.sm, pcard.x + 64, hy + 4, lang.pick("Nom", "Name"), pal.text_muted);
        cv.draw_text(&fonts.sm, pcard.right() - 200, hy + 4, "RAM", pal.text_muted);
        cv.draw_text(&fonts.sm, pcard.right() - 122, hy + 4, lang.pick("Capacités", "Caps"), pal.text_muted);

        let list_r = Rect::new(pcard.x, hy + 28, pcard.w, (pcard.bottom() - (hy + 28) - 6).max(28));
        let mut ry = list_r.y;
        let max_rows = (list_r.h / 28).max(1) as usize;
        let max_proc_off = t.processes.len().saturating_sub(max_rows);
        let eff_proc = self.scroll_proc.min(max_proc_off);
        for p in t.processes.iter().skip(eff_proc).take(max_rows) {
            let row = Rect::new(pcard.x + 6, ry, pcard.w - 18, 26);
            let sel = self.selected_proc_pid == Some(p.pid);
            if sel {
                cv.fill_rounded_rect(row, 6, pal.accent.with_alpha(0x2A));
            } else if row.contains(mx, my) {
                cv.fill_rounded_rect(row, 6, pal.hover_bg);
            }
            cv.draw_text(&fonts.sm, pcard.x + 14, ry + 5, &format!("{}", p.pid), pal.accent);
            cv.draw_text_ellipsis(&fonts.sm, pcard.x + 64, ry + 5, &p.name, pcard.w - 280, pal.text_primary);
            cv.draw_text(&fonts.sm, pcard.right() - 204, ry + 5, &format!("{} Kio", p.memory_kb), pal.text_secondary);
            let cap_lbl = if p.critical { "Système (protégé)" } else { "Utilisateur" };
            cv.draw_text(&fonts.sm, pcard.right() - 126, ry + 5, cap_lbl, if p.critical { pal.success } else { pal.text_muted });
            ry += 28;
        }
        if let Some(sb) = compute_scrollbar(list_r, (t.processes.len() as i32) * 28, (eff_proc as i32) * 28) {
            draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, pal);
        }

        // Right column: System services live status
        let scard = Rect::new(pcard.right() + gap, c.y + pad, right_w, c.h - pad * 2);
        cv.fill_rounded_rect(scard, 12, pal.card_bg);
        cv.stroke_rounded_rect(scard, 12, pal.border_subtle);
        cv.draw_text(&fonts.md, scard.x + 14, scard.y + 12, lang.pick("Services système (IPC)", "System services (IPC)"), pal.text_primary);

        let slist = Rect::new(scard.x, scard.y + 44, scard.w, (scard.bottom() - (scard.y + 44) - 8).max(27));
        let max_svc = (slist.h / 27).max(1) as usize;
        let max_svc_off = t.services.len().saturating_sub(max_svc);
        let eff_svc = self.scroll_svc.min(max_svc_off);
        let mut sy = slist.y;
        for svc in t.services.iter().skip(eff_svc).take(max_svc) {
            cv.fill_circle_aa(scard.x as f32 + 20.0, sy as f32 + 10.0, 4.0, if svc.online { pal.success } else { pal.danger });
            cv.draw_text_ellipsis(&fonts.sm, scard.x + 32, sy + 2, svc.name, scard.w - 116, pal.text_primary);
            let st = if svc.online { "OK" } else { "OFF" };
            cv.draw_text(&fonts.sm, scard.right() - 48, sy + 2, st, if svc.online { pal.success } else { pal.danger });
            sy += 27;
        }
        if let Some(sb) = compute_scrollbar(slist, (t.services.len() as i32) * 27, (eff_svc as i32) * 27) {
            draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, pal);
        }
    }

    pub(crate) fn click_processes(&mut self, c: Rect, mx: i32, my: i32, t: &TelemetryState) -> AppAction {
        let pad = 16;
        let gap = 12;
        let left_w = ((c.w - pad * 2 - gap) * 62) / 100;
        let right_w = c.w - pad * 2 - gap - left_w;
        let pcard = Rect::new(c.x + pad, c.y + pad, left_w, c.h - pad * 2);
        let scard = Rect::new(pcard.right() + gap, c.y + pad, right_w, c.h - pad * 2);

        let list_r = Rect::new(pcard.x, pcard.y + 68, pcard.w, (pcard.bottom() - (pcard.y + 68) - 6).max(28));
        if let Some(sb) = compute_scrollbar(list_r, (t.processes.len() as i32) * 28, (self.scroll_proc as i32) * 28) {
            if sb.track.inset(-4, 0).contains(mx, my) {
                self.dragging_scrollbar = true;
                self.scroll_proc = (scroll_from_drag_y(sb, my) / 28).max(0) as usize;
                return AppAction::Redraw;
            }
        }
        let slist = Rect::new(scard.x, scard.y + 44, scard.w, (scard.bottom() - (scard.y + 44) - 8).max(27));
        if let Some(sb) = compute_scrollbar(slist, (t.services.len() as i32) * 27, (self.scroll_svc as i32) * 27) {
            if sb.track.inset(-4, 0).contains(mx, my) {
                self.dragging_scrollbar = true;
                self.scroll_svc = (scroll_from_drag_y(sb, my) / 27).max(0) as usize;
                return AppAction::Redraw;
            }
        }

        if let Some(pid) = self.selected_proc_pid {
            if Rect::new(pcard.right() - 148, pcard.y + 8, 134, 26).contains(mx, my) {
                return AppAction::TerminatePid(pid);
            }
        }
        let mut ry = pcard.y + 68;
        let max_rows = ((pcard.bottom() - ry - 8) / 28).max(1) as usize;
        for p in t.processes.iter().skip(self.scroll_proc).take(max_rows) {
            if Rect::new(pcard.x + 6, ry, pcard.w - 18, 26).contains(mx, my) {
                self.selected_proc_pid = Some(p.pid);
                return AppAction::Redraw;
            }
            ry += 28;
        }
        AppAction::None
    }
}
