//! Mouse, marquee, right-click menu, and keyboard interaction handlers for Files.

use crate::apps::files::view::breadcrumb_segments;
use crate::apps::files::{FilesApp, FilesSection, ViewMode, SIDEBAR_PLACES};
use crate::apps::{ctx_cmd, AppAction};
use crate::assets::FontSet;
use crate::i18n::Lang;
use exo_ui::widgets::{compute_scrollbar, scroll_from_drag_y, ContextMenuItem};
use exo_ui::{Clipboard, Rect};

impl FilesApp {
    pub fn handle_scroll(&mut self, body: Rect, delta: i32) -> bool {
        let (_, _, viewport, _, _) = Self::layout(body);
        let content_vp = if self.section == FilesSection::Trash {
            Rect::new(viewport.x, viewport.y + 50, viewport.w, (viewport.h - 50).max(40))
        } else {
            viewport
        };
        let count = self.filtered_indices().len();
        let ch = self.content_height(content_vp, count);
        let max_s = (ch - content_vp.h).max(0);
        if max_s <= 0 {
            self.scroll_y = 0;
            return false;
        }
        let next = (self.scroll_y + delta.saturating_mul(52)).clamp(0, max_s);
        if next != self.scroll_y {
            self.scroll_y = next;
            true
        } else {
            false
        }
    }
    pub fn open_entry(&mut self, idx: usize) -> AppAction {
        let Some(ent) = self.entries.get(idx).cloned() else { return AppAction::None; };
        if ent.is_dir {
            self.navigate(&ent.path);
            AppAction::Redraw
        } else if ent.is_exec {
            AppAction::OpenTerminalAt(self.cwd.clone())
        } else {
            AppAction::OpenFileInEditor(ent.path)
        }
    }

    pub fn handle_mouse_press(
        &mut self,
        body: Rect,
        mx: i32,
        my: i32,
        right_button: bool,
        ctrl: bool,
        shift: bool,
        tick: u64,
        lang: Lang,
        clip: &Clipboard,
        fonts: &FontSet,
    ) -> AppAction {
        let (toolbar, sidebar, viewport, inspector, _) = Self::layout(body);

        if right_button && viewport.contains(mx, my) {
            let content_vp = if self.section == FilesSection::Trash {
                Rect::new(viewport.x, viewport.y + 50, viewport.w, (viewport.h - 50).max(40))
            } else {
                viewport
            };
            let filtered = self.filtered_indices();
            for &(idx, r) in &self.item_rects(content_vp, &filtered) {
                if r.contains(mx, my) {
                    if !self.selected.contains(&idx) {
                        self.selected = vec![idx];
                        self.refresh_inspector();
                    }
                    break;
                }
            }
            let has_sel = !self.selected.is_empty();
            let sel_is_dir = self.selected.len() == 1
                && self.selected.first().and_then(|&i| self.entries.get(i)).map(|e| e.is_dir).unwrap_or(false);
            let items = if self.section == FilesSection::Trash {
                vec![
                    ContextMenuItem::new(ctx_cmd::FILES_RESTORE, lang.pick("Restaurer", "Restore"), "", has_sel),
                    ContextMenuItem::new(ctx_cmd::FILES_DELETE_PERM, lang.pick("Supprimer définitivement", "Delete permanently"), "Suppr", has_sel).with_danger(),
                ]
            } else {
                let mut v = Vec::with_capacity(13);
                if has_sel {
                    v.push(ContextMenuItem::new(ctx_cmd::FILES_OPEN, lang.pick("Ouvrir", "Open"), "Entrée", true));
                }
                if sel_is_dir {
                    v.push(ContextMenuItem::new(ctx_cmd::FILES_OPEN_NEW_WIN, lang.pick("Ouvrir dans une fenêtre", "Open in new window"), "", true));
                }
                v.push(ContextMenuItem::new(ctx_cmd::FILES_OPEN_TERM, lang.pick("Ouvrir Terminal ici", "Open Terminal here"), "", true));
                v.push(ContextMenuItem::new(ctx_cmd::SELECT_ALL, lang.pick("Tout sélectionner", "Select all"), "Ctrl+A", true).with_sep());
                v.push(ContextMenuItem::new(ctx_cmd::COPY, lang.pick("Copier", "Copy"), "Ctrl+C", has_sel));
                v.push(ContextMenuItem::new(ctx_cmd::CUT, lang.pick("Couper", "Cut"), "Ctrl+X", has_sel));
                v.push(ContextMenuItem::new(ctx_cmd::PASTE, lang.pick("Coller", "Paste"), "Ctrl+V", !clip.file_paths.is_empty()));
                v.push(ContextMenuItem::new(ctx_cmd::UNDO, lang.pick("Annuler", "Undo"), "Ctrl+Z", !self.undo_stack.is_empty()));
                v.push(ContextMenuItem::new(ctx_cmd::FILES_NEW_DIR, lang.pick("Nouveau dossier", "New folder"), "", true).with_sep());
                v.push(ContextMenuItem::new(ctx_cmd::FILES_NEW_FILE, lang.pick("Nouveau fichier", "New file"), "", true));
                v.push(ContextMenuItem::new(ctx_cmd::FILES_RENAME, lang.pick("Renommer", "Rename"), "F2", self.selected.len() == 1));
                v.push(ContextMenuItem::new(ctx_cmd::FILES_REFRESH, lang.pick("Actualiser", "Refresh"), "F5", true));
                v.push(ContextMenuItem::new(ctx_cmd::FILES_TRASH, lang.pick("Mettre à la corbeille", "Move to Trash"), "Suppr", has_sel).with_sep().with_danger());
                v
            };
            return AppAction::ShowContextMenu { x: mx, y: my, items };
        }
        if right_button { return AppAction::None; }

        if self.renaming_idx.is_some() { self.commit_rename(); }

        if toolbar.contains(mx, my) {
            return self.handle_toolbar_click(toolbar, mx, my, shift, fonts);
        }
        self.search_focused = false;

        if sidebar.contains(mx, my) {
            let mut sy = sidebar.y + 34;
            for &(_, _, path) in &SIDEBAR_PLACES {
                if Rect::new(sidebar.x + 8, sy, sidebar.w - 16, 28).contains(mx, my) {
                    self.navigate(path);
                    return AppAction::Redraw;
                }
                sy += 30;
            }
            sy += 32;
            if Rect::new(sidebar.x + 8, sy, sidebar.w - 16, 28).contains(mx, my) {
                self.open_trash();
                return AppAction::Redraw;
            }
            sy += 30;
            if Rect::new(sidebar.x + 8, sy, sidebar.w - 16, 28).contains(mx, my) {
                self.section = FilesSection::Snapshots;
                return AppAction::Redraw;
            }
            return AppAction::None;
        }

        if inspector.w > 0 && Rect::new(inspector.x + 14, inspector.bottom() - 44, inspector.w - 28, 30).contains(mx, my) {
            return AppAction::CreateSnapshot(self.cwd.clone());
        }

        if viewport.contains(mx, my) {
            if self.section == FilesSection::Snapshots {
                if Rect::new(viewport.right() - 178, viewport.y + 12, 162, 30).contains(mx, my) {
                    return AppAction::CreateSnapshot(String::from("exofs-ui"));
                }
                return AppAction::None;
            }
            let mut content_vp = viewport;
            if self.section == FilesSection::Trash {
                let banner = Rect::new(viewport.x + 10, viewport.y + 8, viewport.w - 20, 38);
                if Rect::new(banner.right() - 216, banner.y + 5, 96, 28).contains(mx, my) {
                    self.restore_from_trash();
                    return AppAction::Redraw;
                }
                if Rect::new(banner.right() - 112, banner.y + 5, 102, 28).contains(mx, my) {
                    self.empty_trash();
                    return AppAction::Redraw;
                }
                content_vp = Rect::new(viewport.x, viewport.y + 50, viewport.w, (viewport.h - 50).max(40));
            }

            let filtered = self.filtered_indices();
            let ch = self.content_height(content_vp, filtered.len());
            if let Some(sb) = compute_scrollbar(content_vp, ch, self.scroll_y) {
                if sb.track.inset(-4, 0).contains(mx, my) {
                    self.dragging_scrollbar = true;
                    self.scroll_y = scroll_from_drag_y(sb, my);
                    return AppAction::Redraw;
                }
            }
            for &(idx, r) in &self.item_rects(content_vp, &filtered) {
                if r.contains(mx, my) {
                    let is_dbl = self.last_click_item == Some(idx) && tick.saturating_sub(self.last_click_tick) <= 28;
                    self.last_click_tick = tick;
                    self.last_click_item = Some(idx);
                    if is_dbl { return self.open_entry(idx); }
                    if ctrl {
                        if let Some(p) = self.selected.iter().position(|&x| x == idx) { self.selected.remove(p); } else { self.selected.push(idx); }
                    } else if shift {
                        if let Some(a) = self.last_clicked {
                            let (lo, hi) = (a.min(idx), a.max(idx));
                            self.selected = filtered.iter().copied().filter(|&k| k >= lo && k <= hi).collect();
                        } else {
                            self.selected = vec![idx];
                        }
                    } else {
                        self.selected = vec![idx];
                    }
                    self.last_clicked = Some(idx);
                    self.refresh_inspector();
                    return AppAction::Redraw;
                }
            }
            if !ctrl { self.selected.clear(); }
            self.marquee_active = true;
            self.marquee_start = (mx, my);
            self.marquee_cur = (mx, my);
            self.refresh_inspector();
            return AppAction::Redraw;
        }
        AppAction::None
    }

    fn handle_toolbar_click(&mut self, toolbar: Rect, mx: i32, my: i32, shift: bool, fonts: &FontSet) -> AppAction {
        let search_w = 166.min(toolbar.w / 4);
        let search_r = Rect::new(toolbar.right() - search_w - 10, toolbar.y + 8, search_w, 30);
        if search_r.contains(mx, my) {
            self.search_focused = true;
            let bi = fonts.sm.hit_test_byte_index(&self.search.text, mx - (search_r.x + 8));
            self.search.mouse_press(bi, shift);
            return AppAction::Redraw;
        }
        self.search_focused = false;

        if Rect::new(toolbar.x + 10, toolbar.y + 9, 28, 28).contains(mx, my) { self.go_back(); return AppAction::Redraw; }
        if Rect::new(toolbar.x + 42, toolbar.y + 9, 28, 28).contains(mx, my) { self.go_forward(); return AppAction::Redraw; }
        if Rect::new(toolbar.x + 74, toolbar.y + 9, 28, 28).contains(mx, my) { self.go_parent(); return AppAction::Redraw; }
        if Rect::new(toolbar.x + 106, toolbar.y + 9, 28, 28).contains(mx, my) { self.reload(); return AppAction::Redraw; }

        let crumb_x = toolbar.x + 144;
        let crumb_w = (toolbar.w - 154 - (278 + search_w)).max(90);
        let crumb_r = Rect::new(crumb_x, toolbar.y + 8, crumb_w, 30);
        if crumb_r.contains(mx, my) {
            let mut cx = crumb_r.x + 8;
            let segs = breadcrumb_segments(&self.cwd);
            for (si, seg) in segs.iter().enumerate() {
                let sw = fonts.sm.measure_width(seg) + 12;
                if mx >= cx && mx < cx + sw {
                    let target = if si == 0 { String::from("/") } else { format!("/{}", segs[1..=si].join("/")) };
                    self.navigate(&target);
                    return AppAction::Redraw;
                }
                cx += sw + 11;
            }
        }

        let mut qx = crumb_r.right() + 8;
        if Rect::new(qx, toolbar.y + 9, 74, 28).contains(mx, my) { self.create_new_folder(); return AppAction::Redraw; }
        qx += 79;
        if Rect::new(qx, toolbar.y + 9, 68, 28).contains(mx, my) { self.create_new_file(); return AppAction::Redraw; }
        qx += 73;
        if Rect::new(qx, toolbar.y + 9, 50, 28).contains(mx, my) {
            self.view_mode = if self.view_mode == ViewMode::Grid { ViewMode::List } else { ViewMode::Grid };
            self.scroll_y = 0;
            return AppAction::Redraw;
        }
        qx += 55;
        if Rect::new(qx, toolbar.y + 9, 66, 28).contains(mx, my) { self.cycle_sort(); return AppAction::Redraw; }
        AppAction::None
    }

    pub fn handle_mouse_move(&mut self, body: Rect, mx: i32, my: i32, fonts: &FontSet) -> bool {
        if self.dragging_scrollbar {
            let (_, _, viewport, _, _) = Self::layout(body);
            let content_vp = if self.section == FilesSection::Trash {
                Rect::new(viewport.x, viewport.y + 50, viewport.w, (viewport.h - 50).max(40))
            } else {
                viewport
            };
            let count = self.filtered_indices().len();
            let ch = self.content_height(content_vp, count);
            if let Some(sb) = compute_scrollbar(content_vp, ch, self.scroll_y) {
                let next = scroll_from_drag_y(sb, my);
                if next != self.scroll_y {
                    self.scroll_y = next;
                    return true;
                }
            }
            return false;
        }
        if self.search.dragging {
            let (toolbar, _, _, _, _) = Self::layout(body);
            let search_w = 166.min(toolbar.w / 4);
            let search_r = Rect::new(toolbar.right() - search_w - 10, toolbar.y + 8, search_w, 30);
            let bi = fonts.sm.hit_test_byte_index(&self.search.text, mx - (search_r.x + 8));
            self.search.mouse_drag(bi);
            return true;
        }
        if self.marquee_active {
            self.marquee_cur = (mx, my);
            let (_, _, viewport, _, _) = Self::layout(body);
            let content_vp = if self.section == FilesSection::Trash {
                Rect::new(viewport.x, viewport.y + 50, viewport.w, (viewport.h - 50).max(40))
            } else {
                viewport
            };
            let mr = Rect::from_points(self.marquee_start.0, self.marquee_start.1, mx, my);
            let filtered = self.filtered_indices();
            self.selected.clear();
            for &(idx, r) in &self.item_rects(content_vp, &filtered) {
                if !r.intersect(mr).is_empty() { self.selected.push(idx); }
            }
            self.refresh_inspector();
            return true;
        }
        false
    }

    pub fn handle_mouse_release(&mut self) -> bool {
        self.search.mouse_release();
        let was_sb = self.dragging_scrollbar;
        self.dragging_scrollbar = false;
        if self.marquee_active {
            self.marquee_active = false;
            return true;
        }
        was_sb
    }

    pub fn handle_context_action(&mut self, cmd: u32, clip: &mut Clipboard) -> AppAction {
        match cmd {
            ctx_cmd::SELECT_ALL => { self.select_all(); AppAction::Redraw }
            ctx_cmd::COPY => { self.copy_selection(clip, false); AppAction::Redraw }
            ctx_cmd::CUT => { self.copy_selection(clip, true); AppAction::Redraw }
            ctx_cmd::PASTE => { self.paste_into_cwd(clip); AppAction::Redraw }
            ctx_cmd::UNDO => { self.undo(); AppAction::Redraw }
            ctx_cmd::FILES_OPEN => self.selected.first().copied().map(|i| self.open_entry(i)).unwrap_or(AppAction::None),
            ctx_cmd::FILES_OPEN_NEW_WIN => {
                let target = self.selected.first().and_then(|&i| self.entries.get(i))
                    .filter(|e| e.is_dir)
                    .map(|e| e.path.clone())
                    .unwrap_or_else(|| self.cwd.clone());
                AppAction::OpenFilesNewWindow(target)
            }
            ctx_cmd::FILES_NEW_DIR => { self.create_new_folder(); AppAction::Redraw }
            ctx_cmd::FILES_NEW_FILE => { self.create_new_file(); AppAction::Redraw }
            ctx_cmd::FILES_RENAME => { self.begin_rename(); AppAction::Redraw }
            ctx_cmd::FILES_TRASH => { self.move_selected_to_trash(); AppAction::Redraw }
            ctx_cmd::FILES_RESTORE => { self.restore_from_trash(); AppAction::Redraw }
            ctx_cmd::FILES_DELETE_PERM => { self.delete_selected_permanently(); AppAction::Redraw }
            ctx_cmd::FILES_OPEN_TERM => {
                let target = if self.selected.len() == 1 {
                    self.selected.first().and_then(|&i| self.entries.get(i))
                        .filter(|e| e.is_dir)
                        .map(|e| e.path.clone())
                        .unwrap_or_else(|| self.cwd.clone())
                } else {
                    self.cwd.clone()
                };
                AppAction::OpenTerminalAt(target)
            }
            ctx_cmd::FILES_SNAPSHOT => AppAction::CreateSnapshot(self.cwd.clone()),
            ctx_cmd::FILES_REFRESH => { self.reload(); AppAction::Redraw }
            _ => AppAction::None,
        }
    }

    pub fn handle_key(&mut self, code: u16, ch: Option<char>, shortcut: Option<char>, ctrl: bool, shift: bool, clip: &mut Clipboard) -> AppAction {
        if ctrl {
            match shortcut {
                Some('a') => { self.select_all(); return AppAction::Redraw; }
                Some('c') => { self.copy_selection(clip, false); return AppAction::Redraw; }
                Some('x') => { self.copy_selection(clip, true); return AppAction::Redraw; }
                Some('v') => { self.paste_into_cwd(clip); return AppAction::Redraw; }
                Some('z') => { self.undo(); return AppAction::Redraw; }
                Some('y') => {
                    if self.search_focused { self.search.redo(); }
                    else if self.renaming_idx.is_some() { self.rename_field.redo(); }
                    return AppAction::Redraw;
                }
                _ => {}
            }
        }
        if self.renaming_idx.is_some() {
            match code {
                0x0028 => self.commit_rename(),
                0x0029 => self.renaming_idx = None,
                0x002a => self.rename_field.backspace(),
                0x004c => self.rename_field.delete_forward(),
                0x0050 => self.rename_field.move_left(shift),
                0x004f => self.rename_field.move_right(shift),
                _ => if let Some(c) = ch { self.rename_field.insert_char(c); } else { return AppAction::None; },
            }
            return AppAction::Redraw;
        }
        if self.search_focused {
            match code {
                0x0029 => self.search_focused = false,
                0x002a => self.search.backspace(),
                0x004c => self.search.delete_forward(),
                0x0050 => self.search.move_left(shift),
                0x004f => self.search.move_right(shift),
                _ => if let Some(c) = ch { self.search.insert_char(c); } else { return AppAction::None; },
            }
            return AppAction::Redraw;
        }
        let approx_max = ((self.entries.len() as i32) * 96).max(0);
        match code {
            0x0028 => if let Some(&i) = self.selected.first() { return self.open_entry(i); },
            0x002a => { self.go_parent(); return AppAction::Redraw; }
            0x003b => { self.begin_rename(); return AppAction::Redraw; }
            0x003e => { self.reload(); return AppAction::Redraw; }
            0x004c => {
                if self.section == FilesSection::Trash { self.delete_selected_permanently(); }
                else { self.move_selected_to_trash(); }
                return AppAction::Redraw;
            }
            0x0051 => { self.scroll_y = self.scroll_y.saturating_add(48).min(approx_max); return AppAction::Redraw; }
            0x0052 => { self.scroll_y = self.scroll_y.saturating_sub(48); return AppAction::Redraw; }
            0x004e => { self.scroll_y = self.scroll_y.saturating_add(240).min(approx_max); return AppAction::Redraw; }
            0x004b => { self.scroll_y = self.scroll_y.saturating_sub(240); return AppAction::Redraw; }
            _ => {}
        }
        AppAction::None
    }
}
