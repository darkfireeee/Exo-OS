//! # Éditeur de texte — Line numbers, Find bar (`Ctrl+F`), syntax tinting, System File Picker (Spec §4.1).

use crate::apps::files::{default_writable_dir, is_read_only_zone, USER_NOTES};
use crate::apps::{ctx_cmd, draw_textfield, AppAction};
use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::widgets::{
    compute_scrollbar, draw_button, draw_scrollbar, scroll_from_drag_y, ContextMenuItem,
};
use exo_ui::{Canvas, Clipboard, Color, Rect, TextField, Theme, ThemeMode};

pub struct EditorApp {
    pub file_path: String,
    pub path_field: TextField,
    pub path_focused: bool,
    pub editor: TextField,
    pub find_open: bool,
    pub find_field: TextField,
    pub status: String,
    pub scroll_y: i32,
    pub dragging_scrollbar: bool,
    last_area_h: i32,
    last_lh: i32,
    last_click_tick: u64,
    last_click_pos: (i32, i32),
    click_streak: u8,
}

impl EditorApp {
    pub fn new(path: Option<&str>) -> Self {
        let notes_dir = default_writable_dir(USER_NOTES);
        let default_path = format!("{}/note.txt", notes_dir);
        let mut app = Self {
            file_path: default_path.clone(),
            path_field: TextField::with_text(&default_path),
            path_focused: false,
            editor: TextField::with_text("// ExoOS Text Editor\nSelect this text with the mouse or use Ctrl+A, Ctrl+C, Ctrl+V, Ctrl+Z, Ctrl+S.\n"),
            find_open: false,
            find_field: TextField::new(),
            status: format!("Ready (saving to {})", notes_dir),
            scroll_y: 0,
            dragging_scrollbar: false,
            last_area_h: 420,
            last_lh: 20,
            last_click_tick: 0,
            last_click_pos: (0, 0),
            click_streak: 0,
        };
        if let Some(p) = path {
            app.load_file(p);
        }
        app
    }

    pub fn load_file(&mut self, path: &str) {
        self.file_path = String::from(path);
        self.path_field = TextField::with_text(path);
        self.path_focused = false;
        self.scroll_y = 0;
        match std::fs::read_to_string(path) {
            Ok(content) => {
                self.editor = TextField::with_text(&content);
                self.status = format!("Opened: {}", path);
            }
            Err(_) => {
                self.editor = TextField::with_text("# New ExoOS File\n");
                self.status = format!("Ready (new): {}", path);
            }
        }
    }

    #[inline]
    fn line_count(&self) -> usize {
        self.editor.text.bytes().filter(|&b| b == b'\n').count() + 1
    }

    #[inline]
    fn content_height(&self, lh: i32) -> i32 {
        (self.line_count() as i32).saturating_mul(lh).saturating_add(24)
    }

    #[inline]
    fn max_scroll(&self, area_h: i32, lh: i32) -> i32 {
        (self.content_height(lh) - area_h).max(0)
    }

    fn ensure_cursor_visible(&mut self) {
        let lh = self.last_lh.max(14);
        let area_h = self.last_area_h.max(80);
        let cur = self.editor.cursor.min(self.editor.text.len());
        let row = self.editor.text[..cur].bytes().filter(|&b| b == b'\n').count() as i32;
        let top_px = row.saturating_mul(lh);
        let bot_px = top_px + lh + 16;
        if top_px < self.scroll_y {
            self.scroll_y = top_px;
        } else if bot_px > self.scroll_y + area_h {
            self.scroll_y = bot_px - area_h;
        }
        self.scroll_y = self.scroll_y.clamp(0, self.max_scroll(area_h, lh));
    }

    pub fn handle_scroll(&mut self, body: Rect, delta: i32, fonts: &FontSet) -> bool {
        let (_, _, area, _) = self.layout(body);
        let lh = fonts.mono.line_height + 5;
        self.last_area_h = area.h;
        self.last_lh = lh;
        let max_s = self.max_scroll(area.h, lh);
        if max_s <= 0 {
            self.scroll_y = 0;
            return false;
        }
        let next = (self.scroll_y + delta.saturating_mul(lh * 3)).clamp(0, max_s);
        if next != self.scroll_y {
            self.scroll_y = next;
            true
        } else {
            false
        }
    }

    fn resolve_writable_save_path(&self) -> String {
        let raw = self.path_field.text.trim();
        let candidate = if raw.is_empty() {
            self.file_path.as_str()
        } else {
            raw
        };
        // Relative filename -> place inside /home/user/Notes
        if !candidate.starts_with('/') {
            let dir = default_writable_dir(USER_NOTES);
            return format!("{}/{}", dir, candidate);
        }
        // Protect root `/` and read-only system zones by redirecting into /home/user/Notes/<name>
        let parent = candidate.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        if parent.is_empty() || is_read_only_zone(candidate) || is_read_only_zone(parent) {
            let fname = candidate.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("note.txt");
            let dir = default_writable_dir(USER_NOTES);
            return format!("{}/{}", dir, fname);
        }
        String::from(candidate)
    }

    pub fn save(&mut self) {
        let target = self.resolve_writable_save_path();
        if let Some((parent, _)) = target.rsplit_once('/') {
            if !parent.is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        self.file_path = target.clone();
        self.path_field = TextField::with_text(&target);
        self.path_focused = false;
        match std::fs::write(&self.file_path, self.editor.text.as_bytes()) {
            Ok(()) => self.status = format!("Saved: {}", self.file_path),
            Err(_) => {
                // Final fallback to /tmp if target volume is read-only
                let fname = self.file_path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("note.txt");
                let fb = format!("/tmp/{}", fname);
                let _ = std::fs::create_dir_all("/tmp");
                match std::fs::write(&fb, self.editor.text.as_bytes()) {
                    Ok(()) => {
                        self.file_path = fb.clone();
                        self.path_field = TextField::with_text(&fb);
                        self.status = format!("Saved: {}", fb);
                    }
                    Err(_) => self.status = format!("Write error ({})", self.file_path),
                }
            }
        }
    }

    fn layout(&self, body: Rect) -> (Rect, Rect, Rect, Rect) {
        let bar = Rect::new(body.x, body.y, body.w, 40);
        let find = if self.find_open { Rect::new(body.x, bar.bottom(), body.w, 34) } else { Rect::ZERO };
        let status = Rect::new(body.x, body.bottom() - 26, body.w, 26);
        let top = if self.find_open { find.bottom() } else { bar.bottom() };
        let area = Rect::new(body.x, top, body.w, (status.y - top).max(60));
        (bar, find, area, status)
    }

    pub fn draw(&self, cv: &mut Canvas<'_>, body: Rect, mx: i32, my: i32, theme: &Theme, lang: Lang, fonts: &FontSet) {
        let pal = theme.palette();
        let (bar, find, area, status) = self.layout(body);

        cv.fill_rect(bar, pal.sidebar_bg);
        cv.fill_rect(Rect::new(bar.x, bar.bottom() - 1, bar.w, 1), pal.border_subtle);
        let btns = [
            (lang.pick("Ouvrir...", "Open..."), 10, 76),
            (lang.pick("Enregistrer", "Save"), 92, 88),
            (lang.pick("Copier", "Copy"), 186, 62),
            (lang.pick("Coller", "Paste"), 254, 58),
            (lang.pick("Annuler", "Undo"), 318, 66),
            (lang.pick("Chercher", "Find"), 390, 74),
        ];
        for (lbl, dx, bw) in btns {
            let r = Rect::new(bar.x + dx, bar.y + 6, bw, 28);
            draw_button(cv, r, lbl, false, r.contains(mx, my), &pal, &fonts.sm);
        }
        let path_r = Rect::new(bar.x + 470, bar.y + 6, (bar.w - 480).max(80), 28);
        cv.fill_rounded_rect(path_r, 6, pal.input_bg);
        cv.stroke_rounded_rect(path_r, 6, if self.path_focused { pal.accent } else { pal.border_subtle });
        draw_textfield(cv, path_r.inset(8, 3), &self.path_field, "/home/user/Notes/note.txt", self.path_focused, &pal, &fonts.sm);

        if self.find_open {
            cv.fill_rect(find, pal.card_bg);
            cv.fill_rect(Rect::new(find.x, find.bottom() - 1, find.w, 1), pal.border_subtle);
            cv.draw_text(&fonts.sm, find.x + 12, find.y + 9, lang.pick("Rechercher :", "Find:"), pal.text_secondary);
            let fr = Rect::new(find.x + 98, find.y + 4, 240, 26);
            cv.fill_rounded_rect(fr, 6, pal.input_bg);
            cv.stroke_rounded_rect(fr, 6, pal.accent);
            draw_textfield(cv, fr.inset(8, 2), &self.find_field, "texte...", true, &pal, &fonts.sm);
        }

        // Editor area with line-number gutter & simple syntax tinting
        let gutter_w = 44;
        cv.fill_rect(area, pal.input_bg);
        cv.fill_rect(Rect::new(area.x, area.y, gutter_w, area.h), pal.sidebar_bg);
        cv.fill_rect(Rect::new(area.x + gutter_w, area.y, 1, area.h), pal.border_subtle);

        let prev = cv.push_clip(area.inset(4, 6));
        let lh = fonts.mono.line_height + 5;
        let (sel_s, sel_e) = self.editor.selection_range();
        let needle = if self.find_open && !self.find_field.text.is_empty() {
            Some(self.find_field.text.to_ascii_lowercase())
        } else {
            None
        };
        let content_h = self.content_height(lh);
        let max_s = (content_h - area.h).max(0);
        let scroll_y = self.scroll_y.clamp(0, max_s);
        let first_line = (scroll_y / lh).max(0) as usize;
        let mut y = area.y + 10 + (first_line as i32) * lh - scroll_y;
        let mut line_start = 0usize;
        let text_x = area.x + gutter_w + 10;

        let sel_bg = if theme.mode == ThemeMode::Light {
            Color::rgba(0x25, 0x63, 0xEB, 0x52)
        } else {
            Color::rgba(0x3B, 0x82, 0xF6, 0x88)
        };

        // O(visible lines): skip all lines above the viewport and break immediately below it.
        for (line_no, raw_line) in self.editor.text.split('\n').enumerate() {
            let line_end = line_start + raw_line.len();
            if line_no < first_line {
                line_start = line_end + 1;
                continue;
            }
            if y > area.bottom() {
                break;
            }
            cv.draw_text(&fonts.sm, area.x + 8, y + 3, &format!("{:>3}", line_no + 1), pal.text_muted);

            if let Some(ref q) = needle {
                if let Some(pos) = raw_line.to_ascii_lowercase().find(q.as_str()) {
                    let hx0 = text_x + fonts.mono.x_of_byte_index(raw_line, pos);
                    let hx1 = text_x + fonts.mono.x_of_byte_index(raw_line, (pos + q.len()).min(raw_line.len()));
                    cv.fill_rounded_rect(Rect::new(hx0, y, (hx1 - hx0).max(6), lh - 2), 3, pal.warning.with_alpha(0x48));
                }
            }
            if sel_s != sel_e && sel_e > line_start && sel_s <= line_end {
                let hs = sel_s.saturating_sub(line_start).min(raw_line.len());
                let he = sel_e.saturating_sub(line_start).min(raw_line.len());
                let x0 = text_x + fonts.mono.x_of_byte_index(raw_line, hs);
                let mut x1 = text_x + fonts.mono.x_of_byte_index(raw_line, he);
                if sel_e > line_end {
                    x1 = (x1 + 10).min(area.right() - 16);
                }
                if x1 > x0 || sel_e > line_end {
                    cv.fill_rounded_rect(Rect::new(x0, y, (x1 - x0).max(6), lh - 2), 3, sel_bg);
                }
            }

            let tr = raw_line.trim_start();
            let fg = if tr.starts_with("//") || tr.starts_with('#') {
                pal.success
            } else if tr.starts_with("fn ") || tr.starts_with("pub ") || tr.starts_with('[') {
                pal.accent
            } else {
                pal.text_primary
            };
            cv.draw_text(&fonts.mono, text_x, y + 2, raw_line, fg);

            if !self.find_open && !self.path_focused && self.editor.cursor >= line_start && self.editor.cursor <= line_end {
                let cx = text_x + fonts.mono.x_of_byte_index(raw_line, self.editor.cursor - line_start);
                cv.fill_rect(Rect::new(cx, y + 1, 2, lh - 3), Color::rgb(0x60, 0xA5, 0xFA));
            }
            y += lh;
            line_start = line_end + 1;
        }
        cv.clip = prev;
        if let Some(sb) = compute_scrollbar(area, content_h, scroll_y) {
            let hov = sb.track.inset(-3, 0).contains(mx, my);
            draw_scrollbar(cv, sb, hov, self.dragging_scrollbar, &pal);
        }

        cv.fill_rect(status, pal.sidebar_bg);
        cv.fill_rect(Rect::new(status.x, status.y, status.w, 1), pal.border_subtle);
        let status_display = if self.editor.has_selection() {
            let n = self.editor.selected_text().chars().count();
            if lang.is_en() {
                format!("Selected: {} chars • Ctrl+C copy", n)
            } else {
                format!("Sélection : {} car. • Ctrl+C copier", n)
            }
        } else {
            self.status.clone()
        };
        cv.draw_text_ellipsis(&fonts.sm, status.x + 12, status.y + 5, &status_display, status.w - 150, if self.editor.has_selection() { Color::rgb(0x60, 0xA5, 0xFA) } else { pal.text_secondary });
        let cnt = format!("{} {} • UTF-8", self.editor.text.len(), lang.pick("car.", "chars"));
        let cw = fonts.sm.measure_width(&cnt);
        cv.draw_text(&fonts.sm, status.right() - cw - 12, status.y + 5, &cnt, pal.text_muted);
    }

    fn hit_test_byte(&self, area: Rect, mx: i32, my: i32, fonts: &FontSet) -> usize {
        let lh = fonts.mono.line_height + 5;
        let max_s = self.max_scroll(area.h, lh);
        let scroll_y = self.scroll_y.clamp(0, max_s);
        let target_row = ((my - (area.y + 10) + scroll_y).max(0) / lh) as usize;
        let mut line_start = 0usize;
        for (row, raw_line) in self.editor.text.split('\n').enumerate() {
            if row == target_row {
                return line_start + fonts.mono.hit_test_byte_index(raw_line, mx - (area.x + 54));
            }
            line_start += raw_line.len() + 1;
        }
        self.editor.text.len()
    }

    fn select_line_at(&mut self, byte_idx: usize) {
        let idx = byte_idx.min(self.editor.text.len());
        let start = self.editor.text[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let end = self.editor.text[idx..].find('\n').map(|p| idx + p + 1).unwrap_or(self.editor.text.len());
        self.editor.anchor = start;
        self.editor.cursor = end;
        self.editor.dragging = false;
    }

    fn move_vertical(&mut self, delta: i32, shift: bool) {
        let cur = self.editor.cursor.min(self.editor.text.len());
        let mut lines: Vec<(usize, &str)> = Vec::new();
        let mut pos = 0usize;
        for line in self.editor.text.split('\n') {
            lines.push((pos, line));
            pos += line.len() + 1;
        }
        if lines.is_empty() { return; }
        let mut cur_row = lines.len() - 1;
        let mut cur_col_chars = 0usize;
        for (r, &(ls, lstr)) in lines.iter().enumerate() {
            if cur >= ls && cur <= ls + lstr.len() {
                cur_row = r;
                cur_col_chars = lstr[..cur - ls].chars().count();
                break;
            }
        }
        let next_row = (cur_row as i32 + delta).clamp(0, lines.len() as i32 - 1) as usize;
        let (nls, nlstr) = lines[next_row];
        let byte_off = nlstr.char_indices().nth(cur_col_chars).map(|(b, _)| b).unwrap_or(nlstr.len());
        self.editor.cursor = nls + byte_off;
        if !shift { self.editor.anchor = self.editor.cursor; }
    }

    pub fn handle_mouse_press(&mut self, body: Rect, mx: i32, my: i32, right: bool, shift: bool, tick: u64, lang: Lang, clip: &mut Clipboard, fonts: &FontSet) -> AppAction {
        let (bar, find, area, _) = self.layout(body);
        let lh = fonts.mono.line_height + 5;
        self.last_area_h = area.h;
        self.last_lh = lh;
        if right {
            let items = vec![
                ContextMenuItem::new(ctx_cmd::SELECT_ALL, lang.pick("Tout sélectionner", "Select all"), "Ctrl+A", true),
                ContextMenuItem::new(ctx_cmd::COPY, lang.pick("Copier", "Copy"), "Ctrl+C", self.editor.has_selection()),
                ContextMenuItem::new(ctx_cmd::CUT, lang.pick("Couper", "Cut"), "Ctrl+X", self.editor.has_selection()),
                ContextMenuItem::new(ctx_cmd::PASTE, lang.pick("Coller", "Paste"), "Ctrl+V", !clip.text.is_empty()),
                ContextMenuItem::new(ctx_cmd::UNDO, lang.pick("Annuler", "Undo"), "Ctrl+Z", true).with_sep(),
                ContextMenuItem::new(ctx_cmd::REDO, lang.pick("Rétablir", "Redo"), "Ctrl+Y", true),
                ContextMenuItem::new(ctx_cmd::EDITOR_OPEN, lang.pick("Ouvrir un fichier...", "Open file..."), "Ctrl+O", true).with_sep(),
                ContextMenuItem::new(ctx_cmd::EDITOR_SAVE, lang.pick("Enregistrer", "Save"), "Ctrl+S", true),
            ];
            return AppAction::ShowContextMenu { x: mx, y: my, items };
        }
        if bar.contains(mx, my) {
            if Rect::new(bar.x + 10, bar.y + 6, 76, 28).contains(mx, my) {
                self.path_focused = false;
                return AppAction::RequestFilePicker { app_name: String::from(lang.pick("Éditeur de texte", "Text Editor")) };
            }
            if Rect::new(bar.x + 92, bar.y + 6, 88, 28).contains(mx, my) { self.save(); return AppAction::Redraw; }
            if Rect::new(bar.x + 186, bar.y + 6, 62, 28).contains(mx, my) { self.editor.copy(clip); return AppAction::Redraw; }
            if Rect::new(bar.x + 254, bar.y + 6, 58, 28).contains(mx, my) { self.editor.paste(clip); self.ensure_cursor_visible(); return AppAction::Redraw; }
            if Rect::new(bar.x + 318, bar.y + 6, 66, 28).contains(mx, my) { self.editor.undo(); self.ensure_cursor_visible(); return AppAction::Redraw; }
            if Rect::new(bar.x + 390, bar.y + 6, 74, 28).contains(mx, my) { self.find_open = !self.find_open; self.path_focused = false; return AppAction::Redraw; }
            let path_r = Rect::new(bar.x + 470, bar.y + 6, (bar.w - 480).max(80), 28);
            if path_r.contains(mx, my) {
                self.path_focused = true;
                self.find_open = false;
                let bi = fonts.sm.hit_test_byte_index(&self.path_field.text, mx - (path_r.x + 8));
                self.path_field.mouse_press(bi, shift);
                return AppAction::Redraw;
            }
        }
        self.path_focused = false;
        if self.find_open && find.contains(mx, my) {
            let fr = Rect::new(find.x + 98, find.y + 4, 240, 26);
            if fr.contains(mx, my) {
                let bi = fonts.sm.hit_test_byte_index(&self.find_field.text, mx - (fr.x + 8));
                self.find_field.mouse_press(bi, shift);
            }
            return AppAction::Redraw;
        }
        if area.contains(mx, my) {
            let ch = self.content_height(lh);
            if let Some(sb) = compute_scrollbar(area, ch, self.scroll_y) {
                if sb.track.inset(-4, 0).contains(mx, my) {
                    self.dragging_scrollbar = true;
                    self.scroll_y = scroll_from_drag_y(sb, my);
                    return AppAction::Redraw;
                }
            }
            self.find_open = false;
            let bi = self.hit_test_byte(area, mx, my, fonts);
            let near = (mx - self.last_click_pos.0).abs() <= 6 && (my - self.last_click_pos.1).abs() <= 6;
            if near && tick.saturating_sub(self.last_click_tick) <= 28 {
                self.click_streak = (self.click_streak + 1).min(3);
            } else {
                self.click_streak = 1;
            }
            self.last_click_tick = tick;
            self.last_click_pos = (mx, my);
            match self.click_streak {
                2 => {
                    self.editor.select_word_at(bi);
                    self.editor.dragging = false;
                }
                3 => self.select_line_at(bi),
                _ => self.editor.mouse_press(bi, shift),
            }
            return AppAction::Redraw;
        }
        AppAction::None
    }

    pub fn handle_mouse_move(&mut self, body: Rect, mx: i32, my: i32, fonts: &FontSet) -> bool {
        if self.dragging_scrollbar {
            let (_, _, area, _) = self.layout(body);
            let lh = fonts.mono.line_height + 5;
            self.last_area_h = area.h;
            self.last_lh = lh;
            if let Some(sb) = compute_scrollbar(area, self.content_height(lh), self.scroll_y) {
                let next = scroll_from_drag_y(sb, my);
                if next != self.scroll_y {
                    self.scroll_y = next;
                    return true;
                }
            }
            return false;
        }
        if self.path_focused && self.path_field.dragging {
            let (bar, _, _, _) = self.layout(body);
            let path_r = Rect::new(bar.x + 470, bar.y + 6, (bar.w - 480).max(80), 28);
            let bi = fonts.sm.hit_test_byte_index(&self.path_field.text, mx - (path_r.x + 8));
            self.path_field.mouse_drag(bi);
            return true;
        }
        if self.find_open && self.find_field.dragging {
            let (_, find, _, _) = self.layout(body);
            let fr = Rect::new(find.x + 98, find.y + 4, 240, 26);
            let bi = fonts.sm.hit_test_byte_index(&self.find_field.text, mx - (fr.x + 8));
            self.find_field.mouse_drag(bi);
            return true;
        }
        if self.editor.dragging {
            let (_, _, area, _) = self.layout(body);
            let lh = fonts.mono.line_height + 5;
            let max_s = self.max_scroll(area.h, lh);
            if my < area.y + 12 {
                self.scroll_y = (self.scroll_y - lh).clamp(0, max_s);
            } else if my > area.bottom() - 12 {
                self.scroll_y = (self.scroll_y + lh).clamp(0, max_s);
            }
            let bi = self.hit_test_byte(area, mx, my, fonts);
            self.editor.mouse_drag(bi);
            return true;
        }
        false
    }

    pub fn handle_mouse_release(&mut self) {
        self.dragging_scrollbar = false;
        self.editor.mouse_release();
        self.find_field.mouse_release();
        self.path_field.mouse_release();
    }

    pub fn handle_context_action(&mut self, cmd: u32, clip: &mut Clipboard) -> AppAction {
        match cmd {
            ctx_cmd::SELECT_ALL => { self.editor.select_all(); AppAction::Redraw }
            ctx_cmd::COPY => { self.editor.copy(clip); AppAction::Redraw }
            ctx_cmd::CUT => { self.editor.cut(clip); self.ensure_cursor_visible(); AppAction::Redraw }
            ctx_cmd::PASTE => { self.editor.paste(clip); self.ensure_cursor_visible(); AppAction::Redraw }
            ctx_cmd::UNDO => { self.editor.undo(); self.ensure_cursor_visible(); AppAction::Redraw }
            ctx_cmd::REDO => { self.editor.redo(); self.ensure_cursor_visible(); AppAction::Redraw }
            ctx_cmd::EDITOR_SAVE => { self.save(); AppAction::Redraw }
            ctx_cmd::EDITOR_OPEN => AppAction::RequestFilePicker { app_name: String::from("Éditeur de texte") },
            _ => AppAction::None,
        }
    }

    pub fn handle_key(&mut self, code: u16, ch: Option<char>, shortcut: Option<char>, ctrl: bool, shift: bool, clip: &mut Clipboard) -> AppAction {
        if ctrl {
            match shortcut {
                Some('f') => { self.find_open = !self.find_open; self.path_focused = false; return AppAction::Redraw; }
                Some('s') => { self.save(); return AppAction::Redraw; }
                Some('o') => return AppAction::RequestFilePicker { app_name: String::from("Éditeur de texte") },
                Some('a') => {
                    if self.path_focused { self.path_field.select_all(); }
                    else if self.find_open { self.find_field.select_all(); }
                    else { self.editor.select_all(); }
                    return AppAction::Redraw;
                }
                Some('c') => {
                    if self.path_focused { self.path_field.copy(clip); }
                    else if self.find_open { self.find_field.copy(clip); }
                    else { self.editor.copy(clip); }
                    return AppAction::Redraw;
                }
                Some('x') => {
                    if self.path_focused { self.path_field.cut(clip); }
                    else if self.find_open { self.find_field.cut(clip); }
                    else { self.editor.cut(clip); self.ensure_cursor_visible(); }
                    return AppAction::Redraw;
                }
                Some('v') => {
                    if self.path_focused { self.path_field.paste(clip); }
                    else if self.find_open { self.find_field.paste(clip); }
                    else { self.editor.paste(clip); self.ensure_cursor_visible(); }
                    return AppAction::Redraw;
                }
                Some('z') => {
                    if self.path_focused { self.path_field.undo(); }
                    else if self.find_open { self.find_field.undo(); }
                    else { self.editor.undo(); self.ensure_cursor_visible(); }
                    return AppAction::Redraw;
                }
                Some('y') => {
                    if self.path_focused { self.path_field.redo(); }
                    else if self.find_open { self.find_field.redo(); }
                    else { self.editor.redo(); self.ensure_cursor_visible(); }
                    return AppAction::Redraw;
                }
                _ => {}
            }
        }
        if self.path_focused {
            match code {
                0x0028 => self.save(),
                0x0029 => {
                    self.path_field = TextField::with_text(&self.file_path);
                    self.path_focused = false;
                }
                0x002a => self.path_field.backspace(),
                0x004c => self.path_field.delete_forward(),
                0x0050 => self.path_field.move_left(shift),
                0x004f => self.path_field.move_right(shift),
                _ => if let Some(c) = ch { self.path_field.insert_char(c); } else { return AppAction::None; },
            }
            return AppAction::Redraw;
        }
        if self.find_open {
            match code {
                0x0029 => self.find_open = false,
                0x002a => self.find_field.backspace(),
                _ => if let Some(c) = ch { self.find_field.insert_char(c); } else { return AppAction::None; },
            }
            return AppAction::Redraw;
        }
        let page_lines = (self.last_area_h / self.last_lh.max(14)).max(4);
        match code {
            0x0028 => self.editor.insert_char('\n'),
            0x002a => self.editor.backspace(),
            0x004c => self.editor.delete_forward(),
            0x0050 => self.editor.move_left(shift),
            0x004f => self.editor.move_right(shift),
            0x0052 => self.move_vertical(-1, shift),
            0x0051 => self.move_vertical(1, shift),
            0x004b => self.move_vertical(-page_lines, shift),
            0x004e => self.move_vertical(page_lines, shift),
            0x004a => self.editor.move_home(shift),
            0x004d => self.editor.move_end(shift),
            _ => if let Some(c) = ch { self.editor.insert_char(c); } else { return AppAction::None; },
        }
        self.ensure_cursor_visible();
        AppAction::Redraw
    }
}
