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
use crate::telemetry::TelemetryState;
use exo_ui::color::lerp_f32;
use exo_ui::{AnimationMode, Clipboard, ContextMenu, Rect, TextField, Theme};

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
    pub(crate) dirty_x0: i32,
    pub(crate) dirty_y0: i32,
    pub(crate) dirty_x1: i32,
    pub(crate) dirty_y1: i32,
}

impl DesktopShell {
    pub fn new(width: u32, height: u32) -> Self {
        let _ = crate::apps::files::ensure_user_home_dirs();
        let w = width.max(640);
        let h = height.max(480);
        Self {
            width: w,
            height: h,
            fonts: FontSet::load(),
            theme: Theme::new(),
            cfg: SystemConfig::default(),
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
            dirty_x0: i32::MAX,
            dirty_y0: i32::MAX,
            dirty_x1: 0,
            dirty_y1: 0,
        }
    }

    #[inline]
    pub fn has_damage(&self) -> bool {
        self.dirty || self.top_bar_dirty || (self.dirty_x0 < self.dirty_x1 && self.dirty_y0 < self.dirty_y1)
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

    #[inline]
    pub fn mark_dirty_rect(&mut self, r: Rect) {
        let x0 = r.x.clamp(0, self.width as i32);
        let y0 = r.y.clamp(0, self.height as i32);
        let x1 = r.right().clamp(0, self.width as i32);
        let y1 = r.bottom().clamp(0, self.height as i32);
        if x0 < x1 && y0 < y1 {
            if x0 < self.dirty_x0 { self.dirty_x0 = x0; }
            if y0 < self.dirty_y0 { self.dirty_y0 = y0; }
            if x1 > self.dirty_x1 { self.dirty_x1 = x1; }
            if y1 > self.dirty_y1 { self.dirty_y1 = y1; }
        }
    }

    #[inline]
    pub fn mark_dirty_band(&mut self, y0: i32, y1: i32) {
        self.mark_dirty_rect(Rect::new(0, y0, self.width as i32, y1 - y0));
    }

    #[inline]
    pub fn take_dirty_rect(&mut self) -> Option<Rect> {
        if self.dirty {
            self.dirty = false;
            self.top_bar_dirty = false;
            self.dirty_x0 = i32::MAX;
            self.dirty_y0 = i32::MAX;
            self.dirty_x1 = 0;
            self.dirty_y1 = 0;
            return Some(Rect::new(0, 0, self.width as i32, self.height as i32));
        }
        let mut x0 = self.dirty_x0;
        let mut y0 = self.dirty_y0;
        let mut x1 = self.dirty_x1;
        let mut y1 = self.dirty_y1;
        if self.top_bar_dirty {
            x0 = 0;
            y0 = 0;
            x1 = self.width as i32;
            y1 = y1.max(TOP_BAR_H.min(self.height as i32));
        }
        self.top_bar_dirty = false;
        self.dirty_x0 = i32::MAX;
        self.dirty_y0 = i32::MAX;
        self.dirty_x1 = 0;
        self.dirty_y1 = 0;
        if x0 < x1 && y0 < y1 {
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
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
        self.dirty = true;
    }

    /// Periodic ~500 ms telemetry & clock tick (partial band damage when idle or showing Monitor/Settings).
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
        self.telemetry.poll();
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
        let mut rects: Vec<Rect> = Vec::new();
        for w in &mut self.windows {
            if !w.minimized && matches!(w.app, AppInstance::Monitor(_) | AppInstance::Settings(_)) {
                w.invalidate_cache();
                rects.push(Rect::new(w.rect.x - 4, w.rect.y - 4, w.rect.w + 8, w.rect.h + 8));
            }
        }
        for r in rects {
            self.mark_dirty_rect(r);
        }
    }

    /// Smooth ~60 Hz transition step for dock hover magnification, power animation & Snake movement.
    pub fn tick_frame(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.power_transition.is_some() {
            self.advance_power_transition(crate::bench::now_ns());
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
            let dr = self.dock_rect();
            self.mark_dirty_band(dr.y - 44, self.height as i32);
        }
        if self.tick % 9 == 0 {
            let mut game_band = None;
            for w in &mut self.windows {
                if !w.minimized {
                    if let AppInstance::Games(ref mut g) = w.app {
                        if g.tick() {
                            w.invalidate_cache();
                            game_band = Some(w.rect);
                        }
                    }
                }
            }
            if let Some(gr) = game_band {
                self.mark_dirty_rect(gr);
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
            w.focused = false;
            w.invalidate_cache();
        }
        let title = if kind == AppKind::Editor {
            if let Some(p) = param { format!("{} — {}", kind.title(self.cfg.lang), p) }
            else { String::from(kind.title(self.cfg.lang)) }
        } else {
            String::from(kind.title(self.cfg.lang))
        };
        self.windows.push(Window::new(id, kind, title, rect, z, app));
        self.close_popovers();
        self.dirty = true;
        id
    }

    pub fn focus_window(&mut self, win_id: u32) {
        let z = self.next_z;
        self.next_z += 1;
        for w in &mut self.windows {
            if w.id == win_id {
                w.focused = true;
                w.minimized = false;
                w.z_order = z;
                w.invalidate_cache();
            } else {
                if w.focused {
                    w.invalidate_cache();
                }
                w.focused = false;
            }
        }
        self.dirty = true;
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

    pub(crate) fn process_action(&mut self, action: AppAction, source_win: Option<u32>) {
        match action {
            AppAction::None => {}
            AppAction::Redraw => {
                if let Some(wid) = source_win {
                    let mut is_settings = false;
                    let mut wr_opt = None;
                    if let Some(w) = self.windows.iter_mut().find(|w| w.id == wid) {
                        w.invalidate_cache();
                        is_settings = matches!(w.app, AppInstance::Settings(_));
                        wr_opt = Some(w.body_rect());
                    }
                    if is_settings {
                        self.invalidate_all_window_caches();
                        self.dirty = true;
                    } else if let Some(wr) = wr_opt {
                        self.mark_dirty_rect(wr);
                    } else {
                        self.dirty = true;
                    }
                } else {
                    self.dirty = true;
                }
            }
            AppAction::OpenTerminalAt(dir) => { self.spawn_app(AppKind::Terminal, Some(&dir)); }
            AppAction::OpenFileInEditor(path) => { self.spawn_app(AppKind::Editor, Some(&path)); }
            AppAction::OpenApp(kind) => { self.open_or_focus_app(kind); }
            AppAction::OpenFilesAt(dir) => {
                let id = self.open_or_focus_app(AppKind::Files);
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    if let AppInstance::Files(ref mut f) = w.app { f.navigate(&dir); }
                }
            }
            AppAction::OpenFilesNewWindow(dir) => {
                self.spawn_app(AppKind::Files, Some(&dir));
            }
            AppAction::OpenSettingsCategory(idx) => {
                let id = self.open_or_focus_app(AppKind::Settings);
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    if let AppInstance::Settings(ref mut s) = w.app { s.category = SettingsCategory::from_index(idx); }
                }
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
                self.show_toast(msg);
            }
            AppAction::CreateSnapshot(tag) => {
                let snap = self.telemetry.create_exofs_snapshot(&tag);
                self.show_toast(format!("Instantané créé : {}", snap));
            }
            AppAction::RunBenchAll => {
                self.bench.run_all(&self.fonts);
                let msg = format!("ExoBench terminé : {}/100 (Note {})", self.bench.global_score, self.bench.grade());
                self.show_toast(msg);
            }
            AppAction::RunBenchModule(cat) => {
                self.bench.run_module(cat, &self.fonts);
                self.dirty = true;
            }
            AppAction::ExportBenchReport => {
                if self.bench.reports.is_empty() { self.bench.run_all(&self.fonts); }
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
                Ok(()) => self.show_toast(format!("Processus #{} arrêté", pid)),
                Err(e) => self.show_toast(e),
            },
            AppAction::LockScreen => {
                self.close_popovers();
                self.locked = true;
                self.dirty = true;
            }
            AppAction::StatusToast(m) => self.show_toast(m),
            AppAction::Reboot => self.begin_power_transition(PowerRequest::Reboot),
            AppAction::Shutdown => self.begin_power_transition(PowerRequest::Shutdown),
        }
    }
}
