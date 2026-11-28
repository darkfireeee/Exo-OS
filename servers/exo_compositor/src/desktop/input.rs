//! Mouse & keyboard routing, desktop marquee, window drag/resize, and Spec §3.10 global shortcuts.

use crate::apps::{ctx_cmd, AppAction, AppKind};
use crate::assets::{WALLPAPER_NAMES_EN, WALLPAPER_NAMES_FR};
use crate::desktop::render::DESKTOP_SHORTCUTS;
use crate::desktop::state::{DesktopShell, TOP_BAR_H};
use crate::desktop::window::{AppInstance, MIN_WIN_H, MIN_WIN_W};
use exo_syscall_abi as syscall;
use exo_ui::widgets::ContextMenuItem;
use exo_ui::{Rect, TextField, ThemeMode};

impl DesktopShell {
    pub fn on_input_event(&mut self, ev: syscall::InputEventWire) {
        self.tick = self.tick.wrapping_add(1);
        if self.power_transition.is_some() {
            return;
        }
        match ev.device {
            syscall::INPUT_DEVICE_MOUSE => self.on_mouse_event(ev),
            syscall::INPUT_DEVICE_KEYBOARD => self.on_key_event(ev),
            _ => {}
        }
    }

    fn on_mouse_event(&mut self, ev: syscall::InputEventWire) {
        let speed = self.cfg.mouse_speed.clamp(1, 3) as i32;
        match ev.code {
            0x0110 => {
                let dx = if speed == 1 { ev.value as i32 } else { (ev.value as i32 * speed) / 2 };
                let nx = (self.mouse_x + dx).clamp(0, self.width as i32 - 1);
                if nx != self.mouse_x { self.mouse_x = nx; self.handle_pointer_move(); }
            }
            0x0111 => {
                let dy = if speed == 1 { ev.value as i32 } else { (ev.value as i32 * speed) / 2 };
                let ny = (self.mouse_y + dy).clamp(0, self.height as i32 - 1);
                if ny != self.mouse_y { self.mouse_y = ny; self.handle_pointer_move(); }
            }
            0x0112 => {
                let delta = ev.value as i32;
                if delta != 0 {
                    self.handle_mouse_wheel(delta);
                }
            }
            0 => {
                let nx = (self.mouse_x + ev.value as i32).clamp(0, self.width as i32 - 1);
                let ny = (self.mouse_y + (ev.ascii as i8) as i32).clamp(0, self.height as i32 - 1);
                if nx != self.mouse_x || ny != self.mouse_y { self.mouse_x = nx; self.mouse_y = ny; self.handle_pointer_move(); }
            }
            0x0100 | 1 => {
                let is_right = self.cfg.swap_mouse_buttons;
                self.dispatch_mouse_btn(is_right, ev.value != 0);
            }
            0x0101 | 2 => {
                let is_right = !self.cfg.swap_mouse_buttons;
                self.dispatch_mouse_btn(is_right, ev.value != 0);
            }
            _ => {}
        }
    }

    fn handle_mouse_wheel(&mut self, delta: i32) {
        if self.locked || self.dialog.is_some() || self.power_transition.is_some() {
            return;
        }
        let (mx, my) = (self.mouse_x, self.mouse_y);
        let target_id = self.topmost_window_at(mx, my).or_else(|| self.focused_window_id());
        let Some(wid) = target_id else { return; };
        let mut dirty_rect = None;
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
            let body = w.body_rect();
            let scrolled = match &mut w.app {
                AppInstance::Editor(a) => a.handle_scroll(body, delta, &self.fonts),
                AppInstance::Terminal(a) => a.handle_scroll(body, delta, &self.fonts),
                AppInstance::Files(a) => a.handle_scroll(body, delta),
                AppInstance::Settings(a) => a.handle_scroll(body, delta),
                AppInstance::Monitor(a) => a.handle_scroll(body, mx, delta, &self.telemetry, &self.bench),
                AppInstance::Games(_) => false,
            };
            if scrolled {
                w.invalidate_cache();
                dirty_rect = Some(w.body_rect());
            }
        }
        if let Some(dr) = dirty_rect {
            self.mark_dirty_rect(dr);
        }
    }

    fn dispatch_mouse_btn(&mut self, right: bool, pressed: bool) {
        if right {
            if pressed { self.handle_pointer_press(true); }
        } else if pressed && !self.left_down {
            self.left_down = true;
            self.handle_pointer_press(false);
        } else if !pressed && self.left_down {
            self.left_down = false;
            self.handle_pointer_release();
        }
    }

    fn handle_pointer_move(&mut self) {
        let (mx, my) = (self.mouse_x, self.mouse_y);
        if self.locked { self.dirty = true; return; }

        if let Some((wid, sx, sy)) = self.drag_max_win {
            if (mx - sx).abs() > 6 || (my - sy).abs() > 6 {
                self.drag_max_win = None;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                    if w.maximized {
                        let ratio = ((mx - w.rect.x) as f32 / w.rect.w.max(1) as f32).clamp(0.15, 0.85);
                        w.maximized = false;
                        w.rect = w.saved_rect;
                        let ox = (w.rect.w as f32 * ratio) as i32;
                        let oy = 18;
                        w.rect.x = (mx - ox).clamp(-w.rect.w + 80, self.width as i32 - 80);
                        w.rect.y = (my - oy).clamp(TOP_BAR_H, self.height as i32 - 64);
                        self.drag_win = Some((wid, ox, oy));
                        self.dirty = true;
                        return;
                    }
                }
            } else {
                return;
            }
        }

        if let Some((wid, ox, oy)) = self.drag_win {
            let mut dirty_opt = None;
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                if !w.maximized {
                    let old = w.rect;
                    let nx = (mx - ox).clamp(-w.rect.w + 80, self.width as i32 - 80);
                    let ny = (my - oy).clamp(TOP_BAR_H, self.height as i32 - 64);
                    if nx != w.rect.x || ny != w.rect.y {
                        w.rect.x = nx;
                        w.rect.y = ny;
                        let x0 = old.x.min(nx) - 16;
                        let y0 = old.y.min(ny) - 14;
                        let x1 = old.right().max(w.rect.right()) + 16;
                        let y1 = old.bottom().max(w.rect.bottom()) + 22;
                        dirty_opt = Some(Rect::new(x0, y0, x1 - x0, y1 - y0));
                    }
                }
            }
            if let Some(dr) = dirty_opt {
                self.mark_dirty_rect(dr);
            }
            return;
        }
        if let Some((wid, sx, sy, orig)) = self.resize_win {
            let mut dirty_opt = None;
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                let old = w.rect;
                w.rect.w = (orig.w + (mx - sx)).clamp(MIN_WIN_W, self.width as i32 - orig.x);
                w.rect.h = (orig.h + (my - sy)).clamp(MIN_WIN_H, self.height as i32 - orig.y - 24);
                w.invalidate_cache();
                let x0 = old.x.min(w.rect.x) - 16;
                let y0 = old.y.min(w.rect.y) - 12;
                let x1 = old.right().max(w.rect.right()) + 16;
                let y1 = old.bottom().max(w.rect.bottom()) + 22;
                dirty_opt = Some(Rect::new(x0, y0, x1 - x0, y1 - y0));
            }
            if let Some(dr) = dirty_opt {
                self.mark_dirty_rect(dr);
            }
            return;
        }
        if self.desk_marquee_active {
            let old_r = Rect::from_points(self.desk_marquee_start.0, self.desk_marquee_start.1, self.desk_marquee_cur.0, self.desk_marquee_cur.1);
            self.desk_marquee_cur = (mx, my);
            let mr = Rect::from_points(self.desk_marquee_start.0, self.desk_marquee_start.1, mx, my);
            let prev_sel = self.desk_selected.clone();
            self.desk_selected.clear();
            for i in 0..DESKTOP_SHORTCUTS.len() {
                if !Self::desktop_icon_rect(i).intersect(mr).is_empty() { self.desk_selected.push(i); }
            }
            let mut y0 = old_r.y.min(mr.y) - 6;
            let mut y1 = old_r.bottom().max(mr.bottom()) + 6;
            if self.desk_selected != prev_sel {
                y0 = y0.min(TOP_BAR_H);
                y1 = y1.max(TOP_BAR_H + 410);
            }
            self.mark_dirty_band(y0, y1);
            return;
        }
        if self.left_down {
            if let Some(wid) = self.focused_window_id() {
                let mut changed_rect = None;
                let mut full_screen_settings = false;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                    let body = w.body_rect();
                    let changed = match &mut w.app {
                        AppInstance::Files(a) => a.handle_mouse_move(body, mx, my, &self.fonts),
                        AppInstance::Terminal(a) => a.handle_mouse_move(body, mx, my, &self.fonts),
                        AppInstance::Settings(a) => {
                            let c = a.handle_mouse_move(body, mx, my, &mut self.theme);
                            full_screen_settings = c && (a.dragging_brightness || a.dragging_alpha);
                            c
                        }
                        AppInstance::Editor(a) => a.handle_mouse_move(body, mx, my, &self.fonts),
                        AppInstance::Monitor(a) => a.handle_mouse_move(body, mx, my, &self.telemetry),
                        AppInstance::Games(_) => false,
                    };
                    if changed {
                        w.invalidate_cache();
                        changed_rect = Some(w.body_rect());
                    }
                }
                if let Some(wr) = changed_rect {
                    if full_screen_settings {
                        self.invalidate_all_window_caches();
                        self.dirty = true;
                    } else {
                        self.mark_dirty_rect(wr);
                    }
                    return;
                }
            }
        }
        if self.context_menu.open {
            let b = self.context_menu.bounds();
            if b.contains(mx, my) { self.mark_dirty_band(b.y - 8, b.bottom() + 12); }
        } else if self.launcher_open {
            let lr = self.launcher_rect();
            if lr.contains(mx, my) { self.mark_dirty_band(lr.y - 8, lr.bottom() + 12); }
        } else if self.quick_settings_open {
            let qr = self.quick_settings_rect();
            if qr.contains(mx, my) { self.mark_dirty_band(qr.y - 8, qr.bottom() + 12); }
        } else if self.dock_rect().inset(-12, -26).contains(mx, my) {
            let dr = self.dock_rect().inset(-18, -42);
            self.mark_dirty_rect(dr);
        } else if my <= TOP_BAR_H + 4 {
            self.top_bar_dirty = true;
        }
    }

    pub(crate) fn handle_pointer_press(&mut self, right_button: bool) {
        let (mx, my) = (self.mouse_x, self.mouse_y);
        if self.locked {
            if !right_button { self.click_lock_screen(mx, my); }
            return;
        }
        if self.dialog.is_some() {
            if !right_button { self.click_active_dialog(mx, my); }
            return;
        }

        let ctrl = (self.modifiers & syscall::INPUT_MOD_CTRL) != 0;
        let shift = (self.modifiers & syscall::INPUT_MOD_SHIFT) != 0;

        if self.context_menu.open {
            let inside = self.context_menu.bounds().contains(mx, my);
            if !right_button {
                if let Some(cmd) = self.context_menu.hit_test(mx, my) {
                    let tgt = self.context_target_win;
                    self.context_menu.close();
                    self.context_target_win = None;
                    self.dispatch_context_cmd(tgt, cmd);
                    return;
                }
            }
            self.context_menu.close();
            self.context_target_win = None;
            self.dirty = true;
            if !right_button && inside {
                return;
            }
        }

        if !right_button && self.click_quick_settings(mx, my) { return; }
        if !right_button && self.calendar_open {
            let inside = self.calendar_popover_rect().contains(mx, my);
            if !inside { self.calendar_open = false; self.dirty = true; } else { return; }
        }
        if !right_button && self.click_dock(mx, my) { return; }
        if !right_button && self.click_launcher(mx, my, shift) { return; }
        if my < TOP_BAR_H && !right_button { self.click_top_bar(mx, my); return; }

        if let Some(wid) = self.topmost_window_at(mx, my) {
            self.focus_window(wid);
            self.handle_window_click(wid, mx, my, right_button, ctrl, shift);
            return;
        }

        let mut unfocused_rects = Vec::new();
        for w in &mut self.windows {
            if w.focused {
                w.focused = false;
                w.invalidate_cache();
                unfocused_rects.push(w.rect);
            }
        }
        for r in unfocused_rects {
            self.mark_dirty_rect(r);
        }
        if right_button {
            let lang = self.cfg.lang;
            let items = vec![
                ContextMenuItem::new(ctx_cmd::DESK_OPEN_FILES, lang.pick("Ouvrir Fichiers", "Open Files"), "Super+E", true),
                ContextMenuItem::new(ctx_cmd::DESK_OPEN_TERM, lang.pick("Ouvrir un terminal", "Open Terminal"), "Super+T", true),
                ContextMenuItem::new(ctx_cmd::DESK_OPEN_MONITOR, lang.pick("Moniteur système", "System Monitor"), "", true),
                ContextMenuItem::new(ctx_cmd::DESK_OPEN_BENCH, lang.pick("Performances (ExoBench)", "Performance (ExoBench)"), "", true),
                ContextMenuItem::new(ctx_cmd::DESK_OPEN_SETTINGS, lang.pick("Paramètres d'affichage", "Display Settings"), "Super+I", true).with_sep(),
                ContextMenuItem::new(ctx_cmd::DESK_TOGGLE_THEME, lang.pick("Basculer Clair / Sombre", "Toggle Light / Dark"), "", true),
                ContextMenuItem::new(ctx_cmd::DESK_NEXT_WALLPAPER, lang.pick("Changer de fond d'écran", "Next Wallpaper"), "", true),
            ];
            self.context_target_win = None;
            self.context_menu.show(mx, my, items, self.width as i32, self.height as i32);
            self.dirty = true;
            return;
        }

        for (i, item) in DESKTOP_SHORTCUTS.iter().enumerate() {
            if Self::desktop_icon_rect(i).contains(mx, my) {
                let now_ms = crate::bench::now_ns() / 1_000_000;
                let is_dbl = self.last_desk_click_idx == Some(i) && now_ms.saturating_sub(self.last_desk_click_ms) <= 420;
                self.last_desk_click_idx = Some(i);
                self.last_desk_click_ms = now_ms;
                if is_dbl {
                    if let Some(dir) = item.dir_override {
                        self.process_action(AppAction::OpenFilesAt(String::from(dir)), None);
                    } else {
                        self.open_or_focus_app(item.kind);
                    }
                    return;
                }
                if ctrl {
                    if let Some(p) = self.desk_selected.iter().position(|&x| x == i) { self.desk_selected.remove(p); }
                    else { self.desk_selected.push(i); }
                } else {
                    self.desk_selected = vec![i];
                }
                self.mark_dirty_band(TOP_BAR_H, TOP_BAR_H + 420);
                return;
            }
        }
        if !ctrl { self.desk_selected.clear(); }
        self.desk_marquee_active = true;
        self.desk_marquee_start = (mx, my);
        self.desk_marquee_cur = (mx, my);
        self.mark_dirty_band(TOP_BAR_H, TOP_BAR_H + 420);
    }

    fn handle_window_click(&mut self, wid: u32, mx: i32, my: i32, right_button: bool, ctrl: bool, shift: bool) {
        let work_area = self.work_area();
        let mut close_it = false;
        let mut action = AppAction::None;

        if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
            if !right_button {
                if w.close_btn().contains(mx, my) { close_it = true; }
                else if w.min_btn().contains(mx, my) { w.minimized = true; w.focused = false; self.dirty = true; return; }
                else if w.max_btn().contains(mx, my) { w.toggle_maximize(work_area); self.dirty = true; return; }
                else if !w.maximized && w.resize_grip().contains(mx, my) { self.resize_win = Some((wid, mx, my, w.rect)); return; }
                else if w.titlebar_rect().contains(mx, my) {
                    let now_ms = crate::bench::now_ns() / 1_000_000;
                    let near = (mx - self.last_title_click_pos.0).abs() <= 8 && (my - self.last_title_click_pos.1).abs() <= 8;
                    let is_dbl = self.last_title_click_win == Some(wid)
                        && near
                        && now_ms.saturating_sub(self.last_title_click_ms) <= 380;
                    self.last_title_click_win = Some(wid);
                    self.last_title_click_ms = now_ms;
                    self.last_title_click_pos = (mx, my);
                    if is_dbl {
                        w.toggle_maximize(work_area);
                        self.last_title_click_win = None;
                        self.dirty = true;
                    } else if w.maximized {
                        self.drag_max_win = Some((wid, mx, my));
                    } else {
                        self.drag_win = Some((wid, mx - w.rect.x, my - w.rect.y));
                    }
                    return;
                }
            }
            let body = w.body_rect();
            if body.contains(mx, my) && !close_it {
                action = match &mut w.app {
                    AppInstance::Files(a) => a.handle_mouse_press(body, mx, my, right_button, ctrl, shift, self.tick, self.cfg.lang, &self.clipboard, &self.fonts),
                    AppInstance::Terminal(a) => a.handle_mouse_press(body, mx, my, right_button, shift, self.cfg.lang, &self.clipboard, &self.fonts),
                    AppInstance::Monitor(a) => if !right_button { a.handle_mouse_press(body, mx, my, self.cfg.lang, &self.fonts, &self.telemetry, &self.bench) } else { AppAction::None },
                    AppInstance::Settings(a) => if !right_button { a.handle_mouse_press(body, mx, my, &mut self.theme, &mut self.cfg, &mut self.telemetry, &self.fonts) } else { AppAction::None },
                    AppInstance::Editor(a) => a.handle_mouse_press(body, mx, my, right_button, shift, self.tick, self.cfg.lang, &mut self.clipboard, &self.fonts),
                    AppInstance::Games(a) => a.handle_mouse_press(body, mx, my, right_button, self.tick),
                };
            }
        }
        if close_it { self.close_window(wid); return; }
        self.process_action(action, Some(wid));
    }

    pub(crate) fn handle_pointer_release(&mut self) {
        let ended_geom = self.drag_win.take().is_some()
            || self.drag_max_win.take().is_some()
            || self.resize_win.take().is_some();
        if self.desk_marquee_active {
            let mr = Rect::from_points(self.desk_marquee_start.0, self.desk_marquee_start.1, self.desk_marquee_cur.0, self.desk_marquee_cur.1);
            self.desk_marquee_active = false;
            self.mark_dirty_rect(Rect::new(mr.x - 6, mr.y - 6, mr.w + 12, mr.h + 12));
        }
        let mut dirty_bodies = Vec::new();
        for w in &mut self.windows {
            let changed = match &mut w.app {
                AppInstance::Files(a) => a.handle_mouse_release(),
                AppInstance::Terminal(a) => a.handle_mouse_release(&mut self.clipboard),
                AppInstance::Settings(a) => { a.handle_mouse_release(); false }
                AppInstance::Editor(a) => { a.handle_mouse_release(); false }
                AppInstance::Monitor(a) => { a.handle_mouse_release(); false }
                _ => false,
            };
            if changed {
                w.invalidate_cache();
                dirty_bodies.push(w.body_rect());
            }
        }
        if ended_geom {
            if let Some(fid) = self.focused_window_id() {
                if let Some(w) = self.windows.iter().find(|w| w.id == fid) {
                    let wr = Rect::new(w.rect.x - 12, w.rect.y - 8, w.rect.w + 24, w.rect.h + 22);
                    self.mark_dirty_rect(wr);
                }
            }
        }
        for r in dirty_bodies {
            self.mark_dirty_rect(r);
        }
    }

    fn dispatch_context_cmd(&mut self, target_win: Option<u32>, cmd: u32) {
        if let Some(wid) = target_win {
            let mut action = AppAction::None;
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                action = match &mut w.app {
                    AppInstance::Files(a) => a.handle_context_action(cmd, &mut self.clipboard),
                    AppInstance::Terminal(a) => a.handle_context_action(cmd, &mut self.clipboard),
                    AppInstance::Editor(a) => a.handle_context_action(cmd, &mut self.clipboard),
                    _ => AppAction::None,
                };
            }
            self.process_action(action, Some(wid));
            return;
        }
        match cmd {
            ctx_cmd::DESK_OPEN_FILES => { self.open_or_focus_app(AppKind::Files); }
            ctx_cmd::DESK_OPEN_TERM => { self.open_or_focus_app(AppKind::Terminal); }
            ctx_cmd::DESK_OPEN_MONITOR => { self.open_or_focus_app(AppKind::Monitor); }
            ctx_cmd::DESK_OPEN_BENCH => { self.open_or_focus_app(AppKind::Bench); }
            ctx_cmd::DESK_OPEN_SETTINGS => { self.process_action(AppAction::OpenSettingsCategory(1), None); }
            ctx_cmd::DESK_TOGGLE_THEME => {
                self.theme.mode = if self.theme.mode == ThemeMode::Dark { ThemeMode::Light } else { ThemeMode::Dark };
                self.invalidate_all_window_caches();
                self.dirty = true;
            }
            ctx_cmd::DESK_NEXT_WALLPAPER => {
                self.theme.wallpaper_idx = (self.theme.wallpaper_idx + 1) % 4;
                let n = if self.cfg.lang.is_en() { WALLPAPER_NAMES_EN[self.theme.wallpaper_idx as usize] } else { WALLPAPER_NAMES_FR[self.theme.wallpaper_idx as usize] };
                self.show_toast(n);
            }
            _ => {}
        }
    }

    fn on_key_event(&mut self, ev: syscall::InputEventWire) {
        self.modifiers = ev.modifiers;
        if ev.state != syscall::INPUT_KEY_PRESSED { return; }
        let code = ev.code;
        let ctrl = (ev.modifiers & syscall::INPUT_MOD_CTRL) != 0;
        let shift = (ev.modifiers & syscall::INPUT_MOD_SHIFT) != 0;
        let alt = (ev.modifiers & syscall::INPUT_MOD_ALT) != 0;
        let meta = (ev.modifiers & syscall::INPUT_MOD_META) != 0;
        let ch = self.cfg.keymap.translate_char(code, ev.ascii, shift, ctrl || meta);
        let sc = self.cfg.keymap.logical_shortcut_key(code);

        if self.locked {
            if code == 0x0028 || code == 0x002c { self.locked = false; self.dirty = true; }
            return;
        }
        if self.dialog.is_some() {
            if code == 0x0029 { self.dialog = None; self.dirty = true; }
            return;
        }

        // Spec §3.10 Global Shortcuts
        if code == 0x00e3 {
            self.launcher_open = !self.launcher_open;
            if self.launcher_open { self.launcher_search = TextField::new(); }
            self.dirty = true;
            return;
        }
        if meta {
            match sc {
                Some('e') => { self.open_or_focus_app(AppKind::Files); return; }
                Some('t') => { self.open_or_focus_app(AppKind::Terminal); return; }
                Some('i') => { self.open_or_focus_app(AppKind::Settings); return; }
                Some('l') => { self.process_action(AppAction::LockScreen, None); return; }
                Some('d') => { for w in &mut self.windows { w.minimized = true; w.focused = false; } self.dirty = true; return; }
                _ => {}
            }
            if code == 0x0050 || code == 0x004f {
                let wa = self.work_area();
                if let Some(wid) = self.focused_window_id() {
                    if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                        if code == 0x0050 { w.snap_left(wa); } else { w.snap_right(wa); }
                        self.dirty = true;
                        return;
                    }
                }
            }
        }
        if alt && code == 0x002b { self.cycle_window_focus(); return; }
        if ctrl && shift && code == 0x0029 { self.open_or_focus_app(AppKind::Monitor); return; }
        if code == 0x0029 && (self.context_menu.open || self.launcher_open || self.quick_settings_open || self.calendar_open) {
            self.close_popovers();
            self.dirty = true;
            return;
        }

        if self.launcher_open {
            if ctrl {
                match sc {
                    Some('a') => self.launcher_search.select_all(),
                    Some('c') => { self.launcher_search.copy(&mut self.clipboard); }
                    Some('x') => { self.launcher_search.cut(&mut self.clipboard); }
                    Some('v') => { self.launcher_search.paste(&self.clipboard); }
                    Some('z') => { self.launcher_search.undo(); }
                    _ => {}
                }
                self.dirty = true;
                return;
            }
            match code {
                0x0028 => if let Some(first) = self.filtered_launcher_items().first().copied() { self.launch_item(first); },
                0x002a => self.launcher_search.backspace(),
                0x004c => self.launcher_search.delete_forward(),
                _ => if let Some(c) = ch { self.launcher_search.insert_char(c); } else { return; },
            }
            self.dirty = true;
            return;
        }

        if let Some(wid) = self.focused_window_id() {
            let mut action = AppAction::None;
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                action = match &mut w.app {
                    AppInstance::Files(a) => a.handle_key(code, ch, sc, ctrl, shift, &mut self.clipboard),
                    AppInstance::Terminal(a) => a.handle_key(code, ch, sc, ctrl, shift, &mut self.clipboard, &mut self.telemetry, &mut self.bench, &self.fonts),
                    AppInstance::Monitor(a) => a.handle_key(code),
                    AppInstance::Settings(a) => a.handle_key(code, ch, sc, ctrl, shift, &mut self.clipboard),
                    AppInstance::Editor(a) => a.handle_key(code, ch, sc, ctrl, shift, &mut self.clipboard),
                    AppInstance::Games(a) => a.handle_key(code),
                };
            }
            self.process_action(action, Some(wid));
        }
    }
}
