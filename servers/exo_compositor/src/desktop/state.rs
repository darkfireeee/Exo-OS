//! Central `DesktopShell` state, window manager, smooth animations, and action dispatcher.

use crate::apps::editor::EditorApp;
use crate::apps::files::FilesApp;
use crate::apps::games::GamesApp;
use crate::apps::monitor::{MonitorApp, MonitorTab};
use crate::apps::settings::{SettingsApp, SettingsCategory, SystemConfig};
use crate::apps::terminal::TerminalApp;
use crate::apps::{AppAction, AppKind};
use crate::assets::FontSet;
use crate::bench::BenchSuite;
use crate::desktop::dialogs::ActiveDialog;
use crate::desktop::window::{AppInstance, Window};
use crate::i18n::Lang;
use crate::telemetry::TelemetryState;
use exo_ui::color::lerp_f32;
use exo_ui::{AccentPreset, AnimationMode, Clipboard, ContextMenu, Rect, TextField, Theme, ThemeMode};

type VisualStamp = (ThemeMode, AccentPreset, u16, u8, u8, bool, bool, bool, Lang);

#[inline]
fn union_rects(rects: &[Rect]) -> Option<Rect> {
    if rects.is_empty() {
        return None;
    }
    let mut x0 = i32::MAX;
    let mut y0 = i32::MAX;
    let mut x1 = i32::MIN;
    let mut y1 = i32::MIN;
    for r in rects {
        if !r.is_empty() {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.right());
            y1 = y1.max(r.bottom());
        }
    }
    if x0 < x1 && y0 < y1 {
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

pub const TOP_BAR_H: i32 = 34;
pub const DOCK_H: i32 = 62;
pub const DOCK_BOTTOM_MARGIN: i32 = 12;

pub const POWER_ANIM_NS: u64 = 1_350_000_000; // 1.35 s smooth 60 Hz shutdown/reboot transition

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerRequest {
    Reboot,
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
pub struct PowerTransition {
    pub req: PowerRequest,
    pub started_ns: u64,
    pub progress: f32,
    pub final_presented: bool,
}

pub struct DesktopShell {
    pub width: u32,
    pub height: u32,
    pub fonts: FontSet,
    pub theme: Theme,
    pub cfg: SystemConfig,
    pub telemetry: TelemetryState,
    pub bench: BenchSuite,
    pub clipboard: Clipboard,
    pub context_menu: ContextMenu,
    pub(crate) context_target_win: Option<u32>,

    pub windows: Vec<Window>,
    next_win_id: u32,
    next_z: u32,

    pub launcher_open: bool,
    pub launcher_search: TextField,
    pub quick_settings_open: bool,
    pub calendar_open: bool,
    pub locked: bool,
    pub dialog: Option<ActiveDialog>,

    pub mouse_x: i32,
    pub mouse_y: i32,
    pub left_down: bool,
    pub modifiers: u8,
    pub tick: u64,
    pub last_render_us: u64,
    pub measured_fps: u32,
    pub gpu_hw_active: bool,

    // Smooth UI transition states (interpolated dock magnification)
    pub(crate) dock_scales: [f32; 8], // 0 = Start, 1..=7 = DOCK_APPS

    pub(crate) drag_win: Option<(u32, i32, i32)>,
    pub(crate) drag_max_win: Option<(u32, i32, i32)>,
    pub(crate) resize_win: Option<(u32, i32, i32, Rect)>,
    pub(crate) last_title_click_win: Option<u32>,
    pub(crate) last_title_click_ms: u64,
    pub(crate) last_title_click_pos: (i32, i32),

    pub(crate) desk_selected: Vec<usize>,
    pub(crate) desk_marquee_active: bool,
    pub(crate) desk_marquee_start: (i32, i32),
    pub(crate) desk_marquee_cur: (i32, i32),
    pub(crate) last_desk_click_idx: Option<usize>,
    pub(crate) last_desk_click_ms: u64,

    pub(crate) toast_text: String,
    pub(crate) toast_ttl: u32,
    pub(crate) power_transition: Option<PowerTransition>,
    pub(crate) power_request: Option<PowerRequest>,
    pub(crate) wallpaper_cache: Vec<u32>,
    pub(crate) wallpaper_cache_idx: u8,
    pub(crate) backbuf: Vec<u32>,
    pub(crate) dirty: bool,
    pub(crate) top_bar_dirty: bool,
    pub(crate) dirty_rects: Vec<Rect>,
    pub(crate) last_game_tick_ns: u64,
    pub(crate) last_fps_hud_ns: u64,
    pub(crate) last_visual_stamp: VisualStamp,
    pub(crate) last_show_fps: bool,
}

impl DesktopShell {
    pub fn new(width: u32, height: u32) -> Self {
        let _ = crate::apps::files::ensure_user_home_dirs();
        let w = width.max(640);
        let h = height.max(480);
        let theme = Theme::new();
        let mut cfg = SystemConfig::default();
        cfg.screen_w = w;
        cfg.screen_h = h;
        let stamp = (
            theme.mode,
            theme.accent,
            theme.brightness_pct,
            theme.surface_alpha,
            theme.wallpaper_idx,
            theme.blur_enabled,
            theme.high_contrast,
            theme.night_light,
            cfg.lang,
        );
        let now_ns = crate::bench::now_ns();
        Self {
            width: w,
            height: h,
            fonts: FontSet::load(),
            theme,
            cfg,
            telemetry: TelemetryState::new(),
            bench: BenchSuite::new(),
            clipboard: Clipboard::new(),
            context_menu: ContextMenu::new(),
            context_target_win: None,
            windows: Vec::new(), // Zero auto-launched apps on boot
            next_win_id: 1,
            next_z: 1,
            launcher_open: false,
            launcher_search: TextField::new(),
            quick_settings_open: false,
            calendar_open: false,
            locked: false,
            dialog: None,
            mouse_x: (w / 2) as i32,
            mouse_y: (h / 2) as i32,
            left_down: false,
            modifiers: 0,
            tick: 0,
            last_render_us: 0,
            measured_fps: 60,
            gpu_hw_active: false,
            dock_scales: [1.0; 8],
            drag_win: None,
            drag_max_win: None,
            resize_win: None,
            last_title_click_win: None,
            last_title_click_ms: 0,
            last_title_click_pos: (0, 0),
            desk_selected: Vec::new(),
            desk_marquee_active: false,
            desk_marquee_start: (0, 0),
            desk_marquee_cur: (0, 0),
            last_desk_click_idx: None,
            last_desk_click_ms: 0,
            toast_text: String::new(),
            toast_ttl: 0,
            power_transition: None,
            power_request: None,
            wallpaper_cache: Vec::new(),
            wallpaper_cache_idx: u8::MAX,
            backbuf: Vec::new(),
            dirty: true,
            top_bar_dirty: false,
            dirty_rects: Vec::with_capacity(8),
            last_game_tick_ns: now_ns,
            last_fps_hud_ns: now_ns,
            last_visual_stamp: stamp,
            last_show_fps: false,
        }
    }

    #[inline]
    fn current_visual_stamp(&self) -> VisualStamp {
        (
            self.theme.mode,
            self.theme.accent,
            self.theme.brightness_pct,
            self.theme.surface_alpha,
            self.theme.wallpaper_idx,
            self.theme.blur_enabled,
            self.theme.high_contrast,
            self.theme.night_light,
            self.cfg.lang,
        )
    }

    #[inline]
    pub fn has_damage(&self) -> bool {
        self.dirty || self.top_bar_dirty || !self.dirty_rects.is_empty()
    }

    #[inline]
    pub fn is_power_transitioning(&self) -> bool {
        self.power_transition.is_some()
    }

    pub fn begin_power_transition(&mut self, req: PowerRequest) {
        if self.power_transition.is_some() {
            return;
        }
        self.close_popovers();
        self.dialog = None;
        self.locked = false;
        self.drag_win = None;
        self.drag_max_win = None;
        self.resize_win = None;
        self.desk_marquee_active = false;
        self.power_transition = Some(PowerTransition {
            req,
            started_ns: crate::bench::now_ns(),
            progress: 0.0,
            final_presented: false,
        });
        self.dirty = true;
    }

    pub fn advance_power_transition(&mut self, now_ns: u64) {
        if let Some(ref mut pt) = self.power_transition {
            let elapsed = now_ns.saturating_sub(pt.started_ns);
            let p = (elapsed as f32 / POWER_ANIM_NS as f32).clamp(0.0, 1.0);
            pt.progress = p;
            self.dirty = true;
            if pt.progress >= 1.0 && pt.final_presented && self.power_request.is_none() {
                self.power_request = Some(pt.req);
            }
        }
    }

    pub fn on_frame_presented(&mut self) {
        if let Some(ref mut pt) = self.power_transition {
            if pt.progress >= 1.0 && !pt.final_presented {
                pt.final_presented = true;
                self.power_request = Some(pt.req);
            }
        }
    }

    #[inline]
    pub fn needs_kbd_grab(&self) -> bool {
        self.locked
            || self.launcher_open
            || self.dialog.is_some()
            || self.windows.iter().any(|w| !w.minimized && w.focused)
    }

    #[inline]
    pub fn invalidate_all_window_caches(&mut self) {
        for w in &mut self.windows {
            w.invalidate_cache();
        }
    }

    /// Multi-rect damage accumulator: keeps disjoint windows/HUDs separate instead of merging
    /// opposite corners of the screen into a giant 1920x1080 full-screen bounding box.
    pub fn mark_dirty_rect(&mut self, r: Rect) {
        if self.dirty {
            return;
        }
        let x0 = r.x.clamp(0, self.width as i32);
        let y0 = r.y.clamp(0, self.height as i32);
        let x1 = r.right().clamp(0, self.width as i32);
        let y1 = r.bottom().clamp(0, self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let nr = Rect::new(x0, y0, x1 - x0, y1 - y0);
        for existing in &mut self.dirty_rects {
            // Merge if overlapping or touching within 8px and union does not bloat area excessively
            let touch = !existing.inset(-8, -8).intersect(nr).is_empty();
            if touch {
                let ux0 = existing.x.min(nr.x);
                let uy0 = existing.y.min(nr.y);
                let ux1 = existing.right().max(nr.right());
                let uy1 = existing.bottom().max(nr.bottom());
                let u_area = (ux1 - ux0) as i64 * (uy1 - uy0) as i64;
                let sum_area = existing.w as i64 * existing.h as i64 + nr.w as i64 * nr.h as i64;
                if u_area <= sum_area + 40_000 {
                    *existing = Rect::new(ux0, uy0, ux1 - ux0, uy1 - uy0);
                    return;
                }
            }
        }
        if self.dirty_rects.len() < 8 {
            self.dirty_rects.push(nr);
        } else {
            // Coalesce into the rect with smallest area increase
            let mut best_i = 0usize;
            let mut best_cost = i64::MAX;
            for (i, ex) in self.dirty_rects.iter().enumerate() {
                let ux0 = ex.x.min(nr.x);
                let uy0 = ex.y.min(nr.y);
                let ux1 = ex.right().max(nr.right());
                let uy1 = ex.bottom().max(nr.bottom());
                let cost = (ux1 - ux0) as i64 * (uy1 - uy0) as i64 - (ex.w as i64 * ex.h as i64);
                if cost < best_cost {
                    best_cost = cost;
                    best_i = i;
                }
            }
            let ex = &mut self.dirty_rects[best_i];
            let ux0 = ex.x.min(nr.x);
            let uy0 = ex.y.min(nr.y);
            let ux1 = ex.right().max(nr.right());
            let uy1 = ex.bottom().max(nr.bottom());
            *ex = Rect::new(ux0, uy0, ux1 - ux0, uy1 - uy0);
        }
    }

    #[inline]
    pub fn mark_dirty_band(&mut self, y0: i32, y1: i32) {
        self.mark_dirty_rect(Rect::new(0, y0, self.width as i32, y1 - y0));
    }

    /// Take all disjoint dirty rectangles for this frame (1–8 tight rects, or fullscreen on global change).
    pub fn take_dirty_rects(&mut self) -> Vec<Rect> {
        if self.dirty {
            self.dirty = false;
            self.top_bar_dirty = false;
            self.dirty_rects.clear();
            return vec![Rect::new(0, 0, self.width as i32, self.height as i32)];
        }
        if self.top_bar_dirty {
            self.top_bar_dirty = false;
            self.mark_dirty_rect(Rect::new(0, 0, self.width as i32, TOP_BAR_H.min(self.height as i32)));
        }
        core::mem::take(&mut self.dirty_rects)
    }

    #[cfg(test)]
    #[inline]
    pub fn take_dirty_rect(&mut self) -> Option<Rect> {
        let rects = self.take_dirty_rects();
        if rects.is_empty() {
            return None;
        }
        let mut x0 = i32::MAX;
        let mut y0 = i32::MAX;
        let mut x1 = 0;
        let mut y1 = 0;
        for r in rects {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.right());
            y1 = y1.max(r.bottom());
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    #[inline]
    pub fn take_power_request(&mut self) -> Option<PowerRequest> {
        self.power_request.take()
    }

    pub fn show_toast(&mut self, msg: impl Into<String>) {
        if self.cfg.dnd_enabled {
            return;
        }
        self.toast_text = msg.into();
        self.toast_ttl = 8;
        self.mark_dirty_band(TOP_BAR_H, TOP_BAR_H + 64);
    }

    /// Periodic ~500 ms telemetry & clock tick: skips static windows (ExoBench, Display/Personalization)
    /// and 100%-occluded background windows so 6+ open windows stay effortlessly smooth.
    pub fn tick_periodic(&mut self) {
        if self.power_transition.is_some()
            || self.drag_win.is_some()
            || self.resize_win.is_some()
            || self.desk_marquee_active
        {
            return;
        }
        let prev_min = self.telemetry.clock.minute;
        let prev_hr = self.telemetry.clock.hour;
        let needs_full_telemetry = self.calendar_open
            || self.quick_settings_open
            || self.windows.iter().any(|w| {
                !w.minimized
                    && match &w.app {
                        AppInstance::Monitor(m) => m.tab != MonitorTab::Bench,
                        AppInstance::Settings(s) => matches!(
                            s.category,
                            SettingsCategory::About | SettingsCategory::SecurityStorage | SettingsCategory::Network
                        ),
                        _ => false,
                    }
            });
        if needs_full_telemetry {
            self.telemetry.poll();
        } else {
            self.telemetry.poll_clock_only();
        }
        let clock_changed =
            self.telemetry.clock.minute != prev_min || self.telemetry.clock.hour != prev_hr;
        if self.toast_ttl > 0 {
            self.toast_ttl -= 1;
            if self.toast_ttl == 0 {
                self.toast_text.clear();
            }
            self.mark_dirty_band(TOP_BAR_H, TOP_BAR_H + 64);
        }
        if self.calendar_open || self.quick_settings_open {
            self.dirty = true;
            return;
        }
        if clock_changed && !self.locked {
            self.top_bar_dirty = true;
        }
        let mut updates: Vec<(u32, Vec<Rect>)> = Vec::new();
        for w in &self.windows {
            if w.minimized {
                continue;
            }
            let live_update = match &w.app {
                AppInstance::Monitor(m) => m.tab != MonitorTab::Bench,
                AppInstance::Settings(s) => {
                    matches!(
                        s.category,
                        SettingsCategory::About | SettingsCategory::SecurityStorage | SettingsCategory::Network
                    ) || (s.category == SettingsCategory::Locale && clock_changed)
                }
                _ => false,
            };
            if live_update {
                let vis = self.visible_window_rects(w.id, w.body_rect());
                if !vis.is_empty() {
                    updates.push((w.id, vis));
                }
            }
        }
        for (wid, vis) in updates {
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                w.invalidate_cache();
            }
            // Collapse fragmented L-shape visible pieces of ONE window into its tight visible bbox
            // so fb_server copies its scanline band ONCE (render_rect still occlusion-culls exact sub-rects!).
            if let Some(bbox) = union_rects(&vis) {
                self.mark_dirty_rect(bbox);
            }
        }
        if self.theme.show_fps || self.cfg.dev_mode {
            let now_ns = crate::bench::now_ns();
            if now_ns.saturating_sub(self.last_fps_hud_ns) >= 450_000_000 {
                self.last_fps_hud_ns = now_ns;
                self.mark_dirty_rect(self.fps_hud_rect());
            }
        }
    }

    /// Smooth ~60 Hz transition step for dock hover magnification, power animation & real-time Snake movement.
    pub fn tick_frame(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let now_ns = crate::bench::now_ns();
        if self.power_transition.is_some() {
            self.advance_power_transition(now_ns);
            return;
        }
        let speed = match self.theme.animations {
            AnimationMode::Full => 0.30,
            AnimationMode::Reduced => 0.65,
            AnimationMode::Off => 1.0,
        };
        let mut dock_moved = false;
        let sb = self.dock_start_btn();
        let target0 = if !self.locked && sb.contains(self.mouse_x, self.mouse_y) { 1.16 } else { 1.0 };
        if (self.dock_scales[0] - target0).abs() > 0.005 {
            self.dock_scales[0] = lerp_f32(self.dock_scales[0], target0, speed);
            dock_moved = true;
        }
        for i in 0..AppKind::DOCK_APPS.len() {
            let r = self.dock_app_rect(i);
            let t = if !self.locked && r.contains(self.mouse_x, self.mouse_y) { 1.18 } else { 1.0 };
            if (self.dock_scales[i + 1] - t).abs() > 0.005 {
                self.dock_scales[i + 1] = lerp_f32(self.dock_scales[i + 1], t, speed);
                dock_moved = true;
            }
        }
        if dock_moved {
            let dr = self.dock_rect().inset(-18, -42);
            self.mark_dirty_rect(dr);
        }
        // Real monotonic 160 ms Snake cadence (independent of mouse/keyboard event rate) + occlusion check
        if now_ns.saturating_sub(self.last_game_tick_ns) >= 160_000_000 {
            self.last_game_tick_ns = now_ns;
            let mut game_vis: Vec<(u32, Vec<Rect>)> = Vec::new();
            for w in &self.windows {
                if !w.minimized && matches!(w.app, AppInstance::Games(_)) {
                    let vis = self.visible_window_rects(w.id, w.body_rect());
                    if !vis.is_empty() {
                        game_vis.push((w.id, vis));
                    }
                }
            }
            for (wid, vis) in game_vis {
                let mut ticked = false;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                    if let AppInstance::Games(ref mut g) = w.app {
                        if g.tick() {
                            w.invalidate_cache();
                            ticked = true;
                        }
                    }
                }
                if ticked {
                    if let Some(bbox) = union_rects(&vis) {
                        self.mark_dirty_rect(bbox);
                    }
                }
            }
        }
    }

    #[inline]
    pub fn work_area(&self) -> Rect {
        Rect::new(
            0,
            TOP_BAR_H,
            self.width as i32,
            (self.height as i32 - TOP_BAR_H - DOCK_H - DOCK_BOTTOM_MARGIN - 8).max(200),
        )
    }

    pub fn open_or_focus_app(&mut self, kind: AppKind) -> u32 {
        if let Some(pos) = self.windows.iter().position(|w| w.kind == kind) {
            let id = self.windows[pos].id;
            self.windows[pos].minimized = false;
            self.focus_window(id);
            self.close_popovers();
            return id;
        }
        self.spawn_app(kind, None)
    }

    pub fn spawn_app(&mut self, kind: AppKind, param: Option<&str>) -> u32 {
        let (dw, dh) = kind.default_size(self.width as i32, self.height as i32);
        let cascade = ((self.next_win_id.saturating_sub(1) % 6) as i32) * 26;
        let x = ((self.width as i32 - dw) / 2 + cascade - 24).clamp(112, (self.width as i32 - dw - 12).max(16));
        let y = (TOP_BAR_H + 22 + cascade).clamp(TOP_BAR_H + 8, (self.height as i32 - dh - DOCK_H - 22).max(TOP_BAR_H + 8));
        let rect = Rect::new(x, y, dw, dh);
        let id = self.next_win_id;
        self.next_win_id += 1;
        let z = self.next_z;
        self.next_z += 1;

        let app = match kind {
            AppKind::Files => AppInstance::Files(FilesApp::new(param.unwrap_or(""))),
            AppKind::Terminal => AppInstance::Terminal(TerminalApp::new(param.unwrap_or(""))),
            AppKind::Monitor => AppInstance::Monitor(MonitorApp::new(MonitorTab::Overview)),
            AppKind::Bench => AppInstance::Monitor(MonitorApp::new(MonitorTab::Bench)),
            AppKind::Settings => AppInstance::Settings(SettingsApp::new()),
            AppKind::Editor => AppInstance::Editor(EditorApp::new(param)),
            AppKind::Games => AppInstance::Games(GamesApp::new()),
        };

        for w in &mut self.windows {
            if w.focused {
                w.focused = false;
                w.invalidate_cache();
            }
        }
        let title = if kind == AppKind::Editor {
            if let Some(p) = param { format!("{} — {}", kind.title(self.cfg.lang), p) }
            else { String::from(kind.title(self.cfg.lang)) }
        } else {
            String::from(kind.title(self.cfg.lang))
        };
        let win = Window::new(id, kind, title, rect, z, app);
        let padded = Rect::new(win.rect.x - 10, win.rect.y - 6, win.rect.w + 20, win.rect.h + 18);
        self.windows.push(win);
        self.close_popovers();
        self.mark_dirty_rect(padded);
        self.top_bar_dirty = true;
        self.mark_dirty_rect(self.dock_rect().inset(-18, -42));
        id
    }

    pub fn focus_window(&mut self, win_id: u32) {
        // Fast no-op when clicking inside an ALREADY topmost focused window (avoids full-screen repaint!)
        let top_z = self.windows.iter().filter(|w| !w.minimized).map(|w| w.z_order).max().unwrap_or(0);
        if self.windows.iter().any(|w| w.id == win_id && w.focused && !w.minimized && w.z_order == top_z) {
            return;
        }
        let z = self.next_z;
        self.next_z += 1;
        let mut dirty_rects = Vec::new();
        for w in &mut self.windows {
            if w.id == win_id {
                w.focused = true;
                w.minimized = false;
                w.z_order = z;
                w.invalidate_cache();
                dirty_rects.push(Rect::new(w.rect.x - 10, w.rect.y - 6, w.rect.w + 20, w.rect.h + 18));
            } else if w.focused {
                w.focused = false;
                w.invalidate_cache();
                dirty_rects.push(Rect::new(w.rect.x - 2, w.rect.y - 2, w.rect.w + 4, w.rect.h + 4));
            }
        }
        for r in dirty_rects {
            self.mark_dirty_rect(r);
        }
        self.top_bar_dirty = true;
        self.mark_dirty_rect(self.dock_rect().inset(-18, -42));
    }

    pub fn close_window(&mut self, win_id: u32) {
        self.windows.retain(|w| w.id != win_id);
        if self.context_target_win == Some(win_id) {
            self.context_menu.close();
            self.context_target_win = None;
        }
        if let Some(top_id) = self.windows.iter().filter(|w| !w.minimized).max_by_key(|w| w.z_order).map(|w| w.id) {
            self.focus_window(top_id);
        }
        self.dirty = true;
    }

    pub fn cycle_window_focus(&mut self) {
        let mut vis: Vec<(u32, u32)> = self.windows.iter().filter(|w| !w.minimized).map(|w| (w.id, w.z_order)).collect();
        if vis.len() < 2 { return; }
        vis.sort_by_key(|&(_, z)| z);
        self.focus_window(vis[0].0);
    }

    pub(crate) fn close_popovers(&mut self) {
        self.launcher_open = false;
        self.quick_settings_open = false;
        self.calendar_open = false;
        self.context_menu.close();
    }

    pub(crate) fn topmost_window_at(&self, mx: i32, my: i32) -> Option<u32> {
        self.windows.iter().filter(|w| !w.minimized && w.rect.contains(mx, my)).max_by_key(|w| w.z_order).map(|w| w.id)
    }

    pub(crate) fn focused_window_id(&self) -> Option<u32> {
        self.windows.iter().filter(|w| !w.minimized && w.focused).max_by_key(|w| w.z_order).map(|w| w.id)
    }

    fn sync_visual_changes(&mut self, source_win: Option<u32>) {
        let new_stamp = self.current_visual_stamp();
        let global_changed = new_stamp != self.last_visual_stamp;
        let fps_toggled = (self.theme.show_fps || self.cfg.dev_mode) != self.last_show_fps;
        self.last_visual_stamp = new_stamp;
        self.last_show_fps = self.theme.show_fps || self.cfg.dev_mode;

        if global_changed {
            self.invalidate_all_window_caches();
            self.dirty = true;
            return;
        }
        if let Some(wid) = source_win {
            let mut wr_opt = None;
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                w.invalidate_cache();
                wr_opt = Some(w.body_rect());
            }
            if let Some(wr) = wr_opt {
                self.mark_dirty_rect(wr);
            } else {
                self.dirty = true;
            }
        } else {
            self.dirty = true;
        }
        if fps_toggled {
            self.mark_dirty_rect(self.fps_hud_rect());
        }
    }

    pub(crate) fn process_action(&mut self, action: AppAction, source_win: Option<u32>) {
        match action {
            AppAction::None => {}
            AppAction::Redraw => {
                self.sync_visual_changes(source_win);
            }
            AppAction::OpenTerminalAt(dir) => { self.spawn_app(AppKind::Terminal, Some(&dir)); }
            AppAction::OpenFileInEditor(path) => { self.spawn_app(AppKind::Editor, Some(&path)); }
            AppAction::OpenApp(kind) => { self.open_or_focus_app(kind); }
            AppAction::OpenFilesAt(dir) => {
                let id = self.open_or_focus_app(AppKind::Files);
                let mut wr = None;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    if let AppInstance::Files(ref mut f) = w.app { f.navigate(&dir); }
                    w.invalidate_cache();
                    wr = Some(w.body_rect());
                }
                if let Some(r) = wr { self.mark_dirty_rect(r); }
            }
            AppAction::OpenFilesNewWindow(dir) => {
                self.spawn_app(AppKind::Files, Some(&dir));
            }
            AppAction::OpenSettingsCategory(idx) => {
                let id = self.open_or_focus_app(AppKind::Settings);
                let mut wr = None;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    if let AppInstance::Settings(ref mut s) = w.app {
                        s.category = SettingsCategory::from_index(idx);
                        s.scroll_y = 0;
                    }
                    w.invalidate_cache();
                    wr = Some(w.body_rect());
                }
                if let Some(r) = wr { self.mark_dirty_rect(r); }
            }
            AppAction::RequestFilePicker { app_name } => {
                if let Some(wid) = source_win {
                    self.dialog = Some(ActiveDialog::new_file_picker(wid, app_name));
                    self.dirty = true;
                }
            }
            AppAction::RequestAdminElevation { reason } => {
                self.dialog = Some(ActiveDialog::AdminElevation { reason });
                self.dirty = true;
            }
            AppAction::ShowContextMenu { x, y, items } => {
                self.context_target_win = source_win;
                self.context_menu.show(x, y, items, self.width as i32, self.height as i32);
                self.dirty = true;
            }
            AppAction::TriggerPhoenixSwitch => {
                let _ = self.telemetry.trigger_phoenix_switch();
                let msg = self.telemetry.last_phoenix_msg.clone();
                self.sync_visual_changes(source_win);
                self.show_toast(msg);
            }
            AppAction::CreateSnapshot(tag) => {
                let snap = self.telemetry.create_exofs_snapshot(&tag);
                self.sync_visual_changes(source_win);
                self.show_toast(format!("Instantané créé : {}", snap));
            }
            AppAction::RunBenchAll => {
                self.bench.run_all(&self.fonts);
                let mut rects = Vec::new();
                for w in &mut self.windows {
                    if matches!(w.app, AppInstance::Monitor(_)) {
                        w.invalidate_cache();
                        rects.push(w.body_rect());
                    }
                }
                for r in rects { self.mark_dirty_rect(r); }
                let msg = format!("ExoBench terminé : {}/100 (Note {})", self.bench.global_score, self.bench.grade());
                self.show_toast(msg);
            }
            AppAction::RunBenchModule(cat) => {
                self.bench.run_module(cat, &self.fonts);
                let mut rects = Vec::new();
                for w in &mut self.windows {
                    if matches!(w.app, AppInstance::Monitor(_)) {
                        w.invalidate_cache();
                        rects.push(w.body_rect());
                    }
                }
                for r in rects { self.mark_dirty_rect(r); }
            }
            AppAction::ExportBenchReport => {
                if self.bench.reports.is_empty() { self.bench.run_all(&self.fonts); }
                let mut rects = Vec::new();
                for w in &mut self.windows {
                    if matches!(w.app, AppInstance::Monitor(_)) {
                        w.invalidate_cache();
                        rects.push(w.body_rect());
                    }
                }
                for r in rects { self.mark_dirty_rect(r); }
                match self.bench.export_report(&self.cfg.hostname) {
                    Ok(p) => self.show_toast(format!("Rapport exporté : {}", p)),
                    Err(e) => self.show_toast(e),
                }
            }
            AppAction::CopyText(txt) => {
                self.clipboard.set_text(&txt);
                self.show_toast(self.cfg.lang.pick("Copié dans le presse-papiers", "Copied to clipboard"));
            }
            AppAction::TerminatePid(pid) => match self.telemetry.terminate_process(pid) {
                Ok(()) => {
                    self.sync_visual_changes(source_win);
                    self.show_toast(format!("Processus #{} arrêté", pid));
                }
                Err(e) => self.show_toast(e),
            },
            AppAction::LockScreen => {
                self.close_popovers();
                self.locked = true;
                self.dirty = true;
            }
            AppAction::StatusToast(m) => {
                self.sync_visual_changes(source_win);
                self.show_toast(m);
            }
            AppAction::Reboot => self.begin_power_transition(PowerRequest::Reboot),
            AppAction::Shutdown => self.begin_power_transition(PowerRequest::Shutdown),
        }
    }
}
