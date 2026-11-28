//! Monitor tab header, universal scrolling, scrollbar dragging, and input routing.

use crate::apps::monitor::{MonitorApp, MonitorTab};
use crate::apps::AppAction;
use crate::assets::{FontSet, IconId};
use crate::bench::BenchSuite;
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{compute_scrollbar, draw_badge, draw_scrollbar, scroll_from_drag_y};
use exo_ui::{Canvas, Rect, Theme};

impl MonitorApp {
    pub fn handle_scroll(&mut self, body: Rect, mx: i32, delta: i32, t: &TelemetryState, bench: &BenchSuite) -> bool {
        let content = Rect::new(body.x, body.y + 50, body.w, (body.h - 50).max(80));
        match self.tab {
            MonitorTab::Processes => {
                let left_w = ((content.w - 44) * 62) / 100;
                let on_right = mx >= content.x + 16 + left_w;
                if on_right {
                    let vis = ((content.h - 84) / 27).max(1) as usize;
                    let max_s = t.services.len().saturating_sub(vis);
                    let next = if delta > 0 {
                        self.scroll_svc.saturating_add(delta as usize * 2).min(max_s)
                    } else {
                        self.scroll_svc.saturating_sub((-delta) as usize * 2)
                    };
                    if next != self.scroll_svc { self.scroll_svc = next; return true; }
                } else {
                    let vis = ((content.h - 108) / 28).max(1) as usize;
                    let max_s = t.processes.len().saturating_sub(vis);
                    let next = if delta > 0 {
                        self.scroll_proc.saturating_add(delta as usize * 2).min(max_s)
                    } else {
                        self.scroll_proc.saturating_sub((-delta) as usize * 2)
                    };
                    if next != self.scroll_proc { self.scroll_proc = next; return true; }
                }
                false
            }
            MonitorTab::Bench => {
                let mcount = bench.reports.get(self.bench_selected_mod).map(|r| r.metrics.len()).unwrap_or(0);
                let main_h = (content.h - 130).max(120);
                let ch = 68 + (mcount as i32) * 60 + 36;
                let max_s = (ch - main_h).max(0);
                let next = (self.scroll_bench + delta * 48).clamp(0, max_s);
                if next != self.scroll_bench { self.scroll_bench = next; true } else { false }
            }
            MonitorTab::Overview | MonitorTab::LiveGraphs => {
                let max_s = (560 - content.h).max(0);
                let next = (self.scroll_y + delta * 48).clamp(0, max_s);
                if next != self.scroll_y { self.scroll_y = next; true } else { false }
            }
        }
    }

    fn tabs(lang: Lang) -> [(MonitorTab, &'static str); 4] {
        [
            (MonitorTab::Overview, lang.pick("Vue d'ensemble", "Overview")),
            (MonitorTab::LiveGraphs, lang.pick("Graphes", "Live Graphs")),
            (MonitorTab::Processes, lang.pick("Processus & Caps", "Processes & Caps")),
            (MonitorTab::Bench, lang.pick("Performances (ExoBench)", "Benchmarks (ExoBench)")),
        ]
    }

    pub fn draw(
        &self,
        cv: &mut Canvas<'_>,
        body: Rect,
        mx: i32,
        my: i32,
        theme: &Theme,
        lang: Lang,
        fonts: &FontSet,
        t: &TelemetryState,
        bench: &BenchSuite,
    ) {
        let pal = theme.palette();
        let hdr = Rect::new(body.x, body.y, body.w, 50);
        cv.fill_rect(hdr, pal.sidebar_bg);
        cv.fill_rect(Rect::new(hdr.x, hdr.bottom() - 1, hdr.w, 1), pal.border_subtle);

        let ico = if self.tab == MonitorTab::Bench { IconId::Bench } else { IconId::Monitor };
        cv.blit_rgba(hdr.x + 14, hdr.y + 9, 32, 32, ico.rgba_40(), 40, 40, 255);
        let score = t.overall_health_score();
        draw_badge(
            cv,
            hdr.x + 54,
            hdr.y + 13,
            &format!("{} {}/100", lang.pick("Santé", "Health"), score),
            if score >= 85 { pal.success } else { pal.warning },
            &fonts.sm,
        );

        let mut tx = hdr.x + 178;
        for (mt, lbl) in Self::tabs(lang) {
            let tw = fonts.sm.measure_width(lbl) + 20;
            let r = Rect::new(tx, hdr.y + 11, tw, 28);
            let active = self.tab == mt;
            if active {
                cv.fill_rounded_rect(r, 8, pal.accent.with_alpha(0x28));
                cv.stroke_rounded_rect(r, 8, pal.accent);
            } else if r.contains(mx, my) {
                cv.fill_rounded_rect(r, 8, pal.hover_bg);
            }
            cv.draw_text(
                &fonts.sm,
                r.x + 10,
                r.y + 6,
                lbl,
                if active { pal.accent } else { pal.text_secondary },
            );
            tx += tw + 8;
        }

        let content = Rect::new(body.x, hdr.bottom(), body.w, (body.h - hdr.h).max(80));
        let prev = cv.push_clip(content);
        match self.tab {
            MonitorTab::Overview => {
                let sy = self.scroll_y.clamp(0, (560 - content.h).max(0));
                let sc = Rect::new(content.x, content.y - sy, content.w, content.h + sy);
                self.draw_overview(cv, sc, &pal, lang, fonts, t);
            }
            MonitorTab::LiveGraphs => {
                let sy = self.scroll_y.clamp(0, (560 - content.h).max(0));
                let sc = Rect::new(content.x, content.y - sy, content.w, content.h + sy);
                self.draw_live_graphs(cv, sc, &pal, lang, fonts, t);
            }
            MonitorTab::Processes => self.draw_processes(cv, content, mx, my, &pal, lang, fonts, t),
            MonitorTab::Bench => self.draw_bench(cv, content, mx, my, &pal, lang, fonts, bench),
        }
        cv.clip = prev;
        if matches!(self.tab, MonitorTab::Overview | MonitorTab::LiveGraphs) {
            let sy = self.scroll_y.clamp(0, (560 - content.h).max(0));
            if let Some(sb) = compute_scrollbar(content, 560, sy) {
                draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), self.dragging_scrollbar, &pal);
            }
        }
    }

    pub fn handle_mouse_press(
        &mut self,
        body: Rect,
        mx: i32,
        my: i32,
        lang: Lang,
        fonts: &FontSet,
        t: &TelemetryState,
        bench: &BenchSuite,
    ) -> AppAction {
        let hdr = Rect::new(body.x, body.y, body.w, 50);
        if hdr.contains(mx, my) {
            let mut tx = hdr.x + 178;
            for (mt, lbl) in Self::tabs(lang) {
                let tw = fonts.sm.measure_width(lbl) + 20;
                if Rect::new(tx, hdr.y + 11, tw, 28).contains(mx, my) {
                    self.tab = mt;
                    self.scroll_y = 0;
                    self.dragging_scrollbar = false;
                    return AppAction::Redraw;
                }
                tx += tw + 8;
            }
        }
        let content = Rect::new(body.x, hdr.bottom(), body.w, (body.h - hdr.h).max(80));
        if matches!(self.tab, MonitorTab::Overview | MonitorTab::LiveGraphs) {
            if let Some(sb) = compute_scrollbar(content, 560, self.scroll_y) {
                if sb.track.inset(-4, 0).contains(mx, my) {
                    self.dragging_scrollbar = true;
                    self.scroll_y = scroll_from_drag_y(sb, my);
                    return AppAction::Redraw;
                }
            }
        }
        match self.tab {
            MonitorTab::Processes => self.click_processes(content, mx, my, t),
            MonitorTab::Bench => self.click_bench(content, mx, my, bench),
            _ => AppAction::None,
        }
    }

    pub fn handle_mouse_move(&mut self, body: Rect, mx: i32, my: i32, t: &TelemetryState) -> bool {
        if !self.dragging_scrollbar { return false; }
        let content = Rect::new(body.x, body.y + 50, body.w, (body.h - 50).max(80));
        match self.tab {
            MonitorTab::Overview | MonitorTab::LiveGraphs => {
                if let Some(sb) = compute_scrollbar(content, 560, self.scroll_y) {
                    let next = scroll_from_drag_y(sb, my);
                    if next != self.scroll_y { self.scroll_y = next; return true; }
                }
            }
            MonitorTab::Processes => {
                let left_w = ((content.w - 44) * 62) / 100;
                let pcard = Rect::new(content.x + 16, content.y + 16, left_w, content.h - 32);
                let scard = Rect::new(pcard.right() + 12, content.y + 16, content.w - 44 - left_w, content.h - 32);
                if mx >= scard.x {
                    let slist = Rect::new(scard.x, scard.y + 44, scard.w, (scard.bottom() - (scard.y + 44) - 8).max(27));
                    if let Some(sb) = compute_scrollbar(slist, (t.services.len() as i32) * 27, (self.scroll_svc as i32) * 27) {
                        self.scroll_svc = (scroll_from_drag_y(sb, my) / 27).max(0) as usize;
                        return true;
                    }
                } else {
                    let list_r = Rect::new(pcard.x, pcard.y + 68, pcard.w, (pcard.bottom() - (pcard.y + 68) - 6).max(28));
                    if let Some(sb) = compute_scrollbar(list_r, (t.processes.len() as i32) * 28, (self.scroll_proc as i32) * 28) {
                        self.scroll_proc = (scroll_from_drag_y(sb, my) / 28).max(0) as usize;
                        return true;
                    }
                }
            }
            MonitorTab::Bench => {}
        }
        false
    }

    pub fn handle_mouse_release(&mut self) {
        self.dragging_scrollbar = false;
    }

    pub fn handle_key(&mut self, code: u16) -> AppAction {
        match (self.tab, code) {
            (MonitorTab::Processes, 0x0051) => { self.scroll_proc = self.scroll_proc.saturating_add(1); AppAction::Redraw }
            (MonitorTab::Processes, 0x0052) => { self.scroll_proc = self.scroll_proc.saturating_sub(1); AppAction::Redraw }
            (MonitorTab::Processes, 0x004e) => { self.scroll_proc = self.scroll_proc.saturating_add(6); AppAction::Redraw }
            (MonitorTab::Processes, 0x004b) => { self.scroll_proc = self.scroll_proc.saturating_sub(6); AppAction::Redraw }
            (MonitorTab::Bench, 0x0051) => { self.scroll_bench = self.scroll_bench.saturating_add(48); AppAction::Redraw }
            (MonitorTab::Bench, 0x0052) => { self.scroll_bench = self.scroll_bench.saturating_sub(48); AppAction::Redraw }
            (MonitorTab::Bench, 0x004e) => { self.scroll_bench = self.scroll_bench.saturating_add(180); AppAction::Redraw }
            (MonitorTab::Bench, 0x004b) => { self.scroll_bench = self.scroll_bench.saturating_sub(180); AppAction::Redraw }
            (_, 0x0051) => { self.scroll_y = self.scroll_y.saturating_add(48).min(300); AppAction::Redraw }
            (_, 0x0052) => { self.scroll_y = self.scroll_y.saturating_sub(48); AppAction::Redraw }
            (_, 0x004e) => { self.scroll_y = self.scroll_y.saturating_add(180).min(300); AppAction::Redraw }
            (_, 0x004b) => { self.scroll_y = self.scroll_y.saturating_sub(180); AppAction::Redraw }
            _ => AppAction::None,
        }
    }
}
