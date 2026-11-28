//! Terminal rendering, tab switching, mouse text selection, and keyboard handling.

use crate::apps::terminal::{clamp_boundary, TermTab, TerminalApp};
use crate::apps::{ctx_cmd, draw_textfield, AppAction};
use crate::assets::FontSet;
use crate::bench::BenchSuite;
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::widgets::{
    compute_scrollbar, draw_button, draw_scrollbar, scroll_from_drag_y, ContextMenuItem,
};
use exo_ui::{Canvas, Clipboard, Color, Rect, TextField, Theme};

impl TerminalApp {
    fn layout(body: Rect) -> (Rect, Rect, Rect) {
        let tabbar = Rect::new(body.x, body.y, body.w, 32);
        let input_h = 38;
        let out = Rect::new(body.x, tabbar.bottom(), body.w, (body.h - tabbar.h - input_h).max(40));
        let inp = Rect::new(body.x, out.bottom(), body.w, input_h);
        (tabbar, out, inp)
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, body: Rect, mx: i32, my: i32, theme: &Theme, fonts: &FontSet) {
        let pal = theme.palette();
        let (tabbar, out_r, inp_r) = Self::layout(body);

        cv.fill_rect(tabbar, pal.sidebar_bg);
        cv.fill_rect(Rect::new(tabbar.x, tabbar.bottom() - 1, tabbar.w, 1), pal.border_subtle);
        let mut tx = tabbar.x + 8;
        for (i, tab) in self.tabs.iter().enumerate() {
            let tr = Rect::new(tx, tabbar.y + 4, 118, 24);
            let act = i == self.active;
            cv.fill_rounded_rect(tr, 6, if act { pal.accent.with_alpha(0x2A) } else if tr.contains(mx, my) { pal.hover_bg } else { pal.card_bg });
            cv.stroke_rounded_rect(tr, 6, if act { pal.accent } else { pal.border_subtle });
            cv.draw_text_ellipsis(&fonts.sm, tr.x + 8, tr.y + 4, &tab.title, tr.w - 16, if act { pal.accent } else { pal.text_secondary });
            tx += 124;
        }
        if self.tabs.len() < 5 {
            let add_r = Rect::new(tx, tabbar.y + 4, 28, 24);
            draw_button(cv, add_r, "+", false, add_r.contains(mx, my), &pal, &fonts.sm);
        }

        cv.fill_rect(out_r, pal.terminal_bg);
        let tab = self.cur();
        let line_h = fonts.mono.line_height + 4;
        let cap = ((out_r.h - 14) / line_h.max(12)).max(1) as usize;
        let (start, end) = tab.visible_window(out_r.h, line_h);
        let sel = tab.normalized_sel();

        let prev = cv.push_clip(out_r.inset(10, 6));
        let mut y = out_r.y + 8;
        for li in start..end {
            let line = &tab.lines[li];
            if let Some(((l0, b0), (l1, b1))) = sel {
                if li >= l0 && li <= l1 {
                    let s0 = if li == l0 { clamp_boundary(&line.text, b0) } else { 0 };
                    let s1 = if li == l1 { clamp_boundary(&line.text, b1) } else { line.text.len() };
                    let x0 = out_r.x + 12 + fonts.mono.x_of_byte_index(&line.text, s0);
                    let x1 = if li < l1 { out_r.right() - 18 } else { out_r.x + 12 + fonts.mono.x_of_byte_index(&line.text, s1) };
                    cv.fill_rounded_rect(Rect::new(x0, y, (x1 - x0).max(5), line_h - 2), 3, Color::rgba(0x3B, 0x82, 0xF6, 0x88));
                }
            }
            cv.draw_text(&fonts.mono, out_r.x + 12, y, &line.text, line.color);
            y += line_h;
        }
        cv.clip = prev;
        if tab.lines.len() > cap {
            let content_h = (tab.lines.len() as i32) * line_h + 16;
            let max_off = tab.lines.len().saturating_sub(cap);
            let top_lines = max_off.saturating_sub(tab.scroll_offset.min(max_off));
            let scroll_px = (top_lines as i32) * line_h;
            if let Some(sb) = compute_scrollbar(out_r, content_h, scroll_px) {
                draw_scrollbar(cv, sb, sb.track.inset(-3, 0).contains(mx, my), tab.dragging_scrollbar, &pal);
            }
        }

        cv.fill_rect(inp_r, Color::rgb(0x16, 0x19, 0x20));
        cv.fill_rect(Rect::new(inp_r.x, inp_r.y, inp_r.w, 1), Color::rgba(0xFF, 0xFF, 0xFF, 0x1A));
        let prompt = tab.prompt_text();
        let pw = fonts.mono.measure_width(&prompt);
        cv.draw_text(&fonts.mono, inp_r.x + 12, inp_r.y + 10, &prompt, Color::rgb(0x10, 0xB9, 0x81));

        let mut dark_pal = pal;
        dark_pal.text_primary = Color::rgb(0xF3, 0xF4, 0xF6);
        dark_pal.text_muted = Color::rgb(0x6E, 0x76, 0x87);
        let tf_r = Rect::new(inp_r.x + 12 + pw, inp_r.y + 4, (inp_r.w - pw - 24).max(40), inp_r.h - 8);
        draw_textfield(cv, tf_r, &tab.input, "", true, &dark_pal, &fonts.mono);
    }

    pub fn handle_scroll(&mut self, body: Rect, delta: i32, fonts: &FontSet) -> bool {
        let (_, out_r, _) = Self::layout(body);
        let line_h = fonts.mono.line_height + 4;
        let cap = ((out_r.h - 14) / line_h.max(12)).max(1) as usize;
        let tab = self.cur_mut();
        let max_off = tab.lines.len().saturating_sub(cap);
        if max_off == 0 {
            tab.scroll_offset = 0;
            return false;
        }
        // delta > 0 scrolls down (toward bottom / offset 0); delta < 0 scrolls up into history.
        let step = delta.unsigned_abs() as usize * 3;
        let next = if delta > 0 {
            tab.scroll_offset.saturating_sub(step)
        } else {
            tab.scroll_offset.saturating_add(step).min(max_off)
        };
        if next != tab.scroll_offset {
            tab.scroll_offset = next;
            true
        } else {
            false
        }
    }

    pub fn handle_mouse_press(&mut self, body: Rect, mx: i32, my: i32, right: bool, shift: bool, lang: Lang, clip: &Clipboard, fonts: &FontSet) -> AppAction {
        let (tabbar, out_r, inp_r) = Self::layout(body);
        if right {
            let has_sel = self.cur().input.has_selection() || self.cur().normalized_sel().is_some();
            let items = vec![
                ContextMenuItem::new(ctx_cmd::SELECT_ALL, lang.pick("Tout sélectionner", "Select all"), "Ctrl+A", true),
                ContextMenuItem::new(ctx_cmd::COPY, lang.pick("Copier", "Copy"), "Ctrl+C", has_sel || !self.cur().input.text.is_empty()),
                ContextMenuItem::new(ctx_cmd::CUT, lang.pick("Couper", "Cut"), "Ctrl+X", self.cur().input.has_selection()),
                ContextMenuItem::new(ctx_cmd::PASTE, lang.pick("Coller", "Paste"), "Ctrl+V", !clip.text.is_empty()),
                ContextMenuItem::new(ctx_cmd::UNDO, lang.pick("Annuler", "Undo"), "Ctrl+Z", true),
                ContextMenuItem::new(ctx_cmd::TERM_CLEAR, lang.pick("Effacer l'écran", "Clear screen"), "Ctrl+L", true).with_sep(),
            ];
            return AppAction::ShowContextMenu { x: mx, y: my, items };
        }
        if tabbar.contains(mx, my) {
            let mut tx = tabbar.x + 8;
            for i in 0..self.tabs.len() {
                if Rect::new(tx, tabbar.y + 4, 118, 24).contains(mx, my) {
                    self.active = i;
                    return AppAction::Redraw;
                }
                tx += 124;
            }
            if self.tabs.len() < 5 && Rect::new(tx, tabbar.y + 4, 28, 24).contains(mx, my) {
                let cwd = self.cur().cwd.clone();
                let n = self.next_tab_num;
                self.next_tab_num += 1;
                self.tabs.push(TermTab::new(n, &cwd));
                self.active = self.tabs.len() - 1;
                return AppAction::Redraw;
            }
            return AppAction::None;
        }
        if inp_r.contains(mx, my) {
            let tab = self.cur_mut();
            tab.sel_anchor = None;
            tab.sel_cursor = None;
            let pw = fonts.mono.measure_width(&tab.prompt_text());
            let bi = fonts.mono.hit_test_byte_index(&tab.input.text, mx - (inp_r.x + 12 + pw));
            tab.input.mouse_press(bi, shift);
            return AppAction::Redraw;
        }
        if out_r.contains(mx, my) {
            let line_h = fonts.mono.line_height + 4;
            let cap = ((out_r.h - 14) / line_h.max(12)).max(1) as usize;
            let lines_len = self.cur().lines.len();
            if lines_len > cap {
                let content_h = (lines_len as i32) * line_h + 16;
                let max_off = lines_len.saturating_sub(cap);
                let top_lines = max_off.saturating_sub(self.cur().scroll_offset.min(max_off));
                if let Some(sb) = compute_scrollbar(out_r, content_h, (top_lines as i32) * line_h) {
                    if sb.track.inset(-4, 0).contains(mx, my) {
                        let px = scroll_from_drag_y(sb, my);
                        let next_top = ((px + line_h / 2) / line_h).clamp(0, max_off as i32) as usize;
                        let tab = self.cur_mut();
                        tab.dragging_scrollbar = true;
                        tab.scroll_offset = max_off.saturating_sub(next_top);
                        return AppAction::Redraw;
                    }
                }
            }
            if let Some(pos) = self.hit_test_out(out_r, mx, my, fonts) {
                let tab = self.cur_mut();
                tab.sel_anchor = Some(pos);
                tab.sel_cursor = Some(pos);
                tab.selecting_output = true;
                return AppAction::Redraw;
            }
        }
        AppAction::None
    }

    pub fn handle_mouse_move(&mut self, body: Rect, mx: i32, my: i32, fonts: &FontSet) -> bool {
        let (_, out_r, inp_r) = Self::layout(body);
        if self.cur().dragging_scrollbar {
            let line_h = fonts.mono.line_height + 4;
            let cap = ((out_r.h - 14) / line_h.max(12)).max(1) as usize;
            let lines_len = self.cur().lines.len();
            if lines_len > cap {
                let content_h = (lines_len as i32) * line_h + 16;
                let max_off = lines_len.saturating_sub(cap);
                let top_lines = max_off.saturating_sub(self.cur().scroll_offset.min(max_off));
                if let Some(sb) = compute_scrollbar(out_r, content_h, (top_lines as i32) * line_h) {
                    let px = scroll_from_drag_y(sb, my);
                    let next_top = ((px + line_h / 2) / line_h).clamp(0, max_off as i32) as usize;
                    let next_off = max_off.saturating_sub(next_top);
                    let tab = self.cur_mut();
                    if next_off != tab.scroll_offset {
                        tab.scroll_offset = next_off;
                        return true;
                    }
                }
            }
            return false;
        }
        if self.cur().input.dragging {
            let tab = self.cur_mut();
            let pw = fonts.mono.measure_width(&tab.prompt_text());
            let bi = fonts.mono.hit_test_byte_index(&tab.input.text, mx - (inp_r.x + 12 + pw));
            tab.input.mouse_drag(bi);
            return true;
        }
        if self.cur().selecting_output {
            if let Some(pos) = self.hit_test_out(out_r, mx, my, fonts) {
                self.cur_mut().sel_cursor = Some(pos);
                return true;
            }
        }
        false
    }

    pub fn handle_mouse_release(&mut self, clip: &mut Clipboard) -> bool {
        let tab = self.cur_mut();
        let was_sb = tab.dragging_scrollbar;
        tab.dragging_scrollbar = false;
        tab.input.mouse_release();
        if tab.selecting_output {
            tab.selecting_output = false;
            if let Some(txt) = tab.extract_sel() { clip.set_text(&txt); }
            return true;
        }
        was_sb
    }

    fn hit_test_out(&self, out_r: Rect, mx: i32, my: i32, fonts: &FontSet) -> Option<(usize, usize)> {
        let tab = self.cur();
        if tab.lines.is_empty() { return None; }
        let lh = fonts.mono.line_height + 4;
        let (start, end) = tab.visible_window(out_r.h, lh);
        if start >= end { return None; }
        let row = ((my - (out_r.y + 8)).max(0) / lh) as usize;
        let li = (start + row).min(end - 1);
        let bi = fonts.mono.hit_test_byte_index(&tab.lines[li].text, mx - (out_r.x + 12));
        Some((li, bi))
    }

    pub fn handle_context_action(&mut self, cmd: u32, clip: &mut Clipboard) -> AppAction {
        let tab = self.cur_mut();
        match cmd {
            ctx_cmd::SELECT_ALL => {
                if !tab.input.text.is_empty() { tab.input.select_all(); }
                else if !tab.lines.is_empty() {
                    let last = tab.lines.len() - 1;
                    tab.sel_anchor = Some((0, 0));
                    tab.sel_cursor = Some((last, tab.lines[last].text.len()));
                }
                AppAction::Redraw
            }
            ctx_cmd::COPY => {
                if let Some(txt) = tab.extract_sel() { clip.set_text(&txt); } else { tab.input.copy(clip); }
                AppAction::Redraw
            }
            ctx_cmd::CUT => { tab.input.cut(clip); AppAction::Redraw }
            ctx_cmd::PASTE => { tab.input.paste(clip); AppAction::Redraw }
            ctx_cmd::UNDO => { tab.input.undo(); AppAction::Redraw }
            ctx_cmd::TERM_CLEAR => { tab.lines.clear(); tab.scroll_offset = 0; AppAction::Redraw }
            _ => AppAction::None,
        }
    }

    pub fn handle_key(
        &mut self,
        code: u16,
        ch: Option<char>,
        shortcut: Option<char>,
        ctrl: bool,
        shift: bool,
        clip: &mut Clipboard,
        telemetry: &mut TelemetryState,
        bench: &mut BenchSuite,
        fonts: &FontSet,
    ) -> AppAction {
        if ctrl {
            let tab = self.cur_mut();
            match shortcut {
                Some('a') => { tab.input.select_all(); return AppAction::Redraw; }
                Some('c') => {
                    if let Some(txt) = tab.extract_sel() { clip.set_text(&txt); }
                    else if tab.input.has_selection() { tab.input.copy(clip); }
                    else { tab.push_line("^C", Color::rgb(0xF5, 0x9E, 0x0B)); tab.input.set_text(""); }
                    return AppAction::Redraw;
                }
                Some('x') => { tab.input.cut(clip); return AppAction::Redraw; }
                Some('v') => { tab.input.paste(clip); return AppAction::Redraw; }
                Some('z') => { tab.input.undo(); return AppAction::Redraw; }
                Some('y') => { tab.input.redo(); return AppAction::Redraw; }
                Some('l') => { tab.lines.clear(); tab.scroll_offset = 0; return AppAction::Redraw; }
                _ => {}
            }
        }
        match code {
            0x0028 => {
                let cmd = self.cur().input.text.clone();
                let tab = self.cur_mut();
                tab.input = TextField::new();
                tab.scroll_offset = 0;
                return self.execute_command(&cmd, telemetry, bench, fonts);
            }
            0x004b => {
                let tab = self.cur_mut();
                let max_off = tab.lines.len().saturating_sub(6);
                tab.scroll_offset = tab.scroll_offset.saturating_add(8).min(max_off);
            }
            0x004e => {
                let tab = self.cur_mut();
                tab.scroll_offset = tab.scroll_offset.saturating_sub(8);
            }
            0x002a => self.cur_mut().input.backspace(),
            0x004c => self.cur_mut().input.delete_forward(),
            0x0050 => self.cur_mut().input.move_left(shift),
            0x004f => self.cur_mut().input.move_right(shift),
            0x004a => self.cur_mut().input.move_home(shift),
            0x004d => self.cur_mut().input.move_end(shift),
            0x0052 => {
                let tab = self.cur_mut();
                if !tab.history.is_empty() {
                    let i = match tab.history_idx { None => tab.history.len() - 1, Some(0) => 0, Some(k) => k - 1 };
                    tab.history_idx = Some(i);
                    tab.input = TextField::with_text(&tab.history[i]);
                }
            }
            0x0051 => {
                let tab = self.cur_mut();
                if let Some(i) = tab.history_idx {
                    if i + 1 < tab.history.len() {
                        tab.history_idx = Some(i + 1);
                        tab.input = TextField::with_text(&tab.history[i + 1]);
                    } else {
                        tab.history_idx = None;
                        tab.input = TextField::new();
                    }
                }
            }
            _ => if let Some(c) = ch { self.cur_mut().input.insert_char(c); } else { return AppAction::None; },
        }
        AppAction::Redraw
    }
}
