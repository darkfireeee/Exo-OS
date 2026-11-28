//! FilesApp constructors, layout geometry, navigation history, and path utilities.

use crate::apps::files::{ensure_user_home_dirs, FilesApp, FilesSection, SortBy, ViewMode, TRASH_DIR};
use crate::telemetry::inspect_exofs_object;
use exo_ui::{Rect, TextField};

impl FilesApp {
    pub fn new(initial_dir: &str) -> Self {
        let home = ensure_user_home_dirs();
        let start = if initial_dir.is_empty() { home } else { initial_dir };
        let mut app = Self {
            cwd: normalize_path(start),
            section: if start == TRASH_DIR { FilesSection::Trash } else { FilesSection::Directory },
            entries: Vec::new(),
            selected: Vec::new(),
            last_clicked: None,
            view_mode: ViewMode::Grid,
            sort_by: SortBy::Name,
            show_hidden: false,
            scroll_y: 0,
            dragging_scrollbar: false,
            history_back: Vec::new(),
            history_fwd: Vec::new(),
            search: TextField::new(),
            search_focused: false,
            renaming_idx: None,
            rename_field: TextField::new(),
            marquee_active: false,
            marquee_start: (0, 0),
            marquee_cur: (0, 0),
            inspector_info: None,
            inspector_path: String::new(),
            status_line: String::from("Ready"),
            undo_stack: Vec::new(),
            last_click_tick: 0,
            last_click_item: None,
        };
        app.reload();
        app
    }

    pub(crate) fn layout(body: Rect) -> (Rect, Rect, Rect, Rect, Rect) {
        let tb_h = 46;
        let sb_h = 28;
        let side_w = 178.min(body.w / 4);
        let insp_w = if body.w >= 760 { 226 } else { 0 };
        let toolbar = Rect::new(body.x, body.y, body.w, tb_h);
        let statusbar = Rect::new(body.x, body.bottom() - sb_h, body.w, sb_h);
        let mid_h = (body.h - tb_h - sb_h).max(80);
        let sidebar = Rect::new(body.x, toolbar.bottom(), side_w, mid_h);
        let inspector = Rect::new(body.right() - insp_w, toolbar.bottom(), insp_w, mid_h);
        let viewport = Rect::new(sidebar.right(), toolbar.bottom(), (body.w - side_w - insp_w).max(120), mid_h);
        (toolbar, sidebar, viewport, inspector, statusbar)
    }

    pub fn navigate(&mut self, target: &str) {
        let norm = normalize_path(target);
        if norm != self.cwd {
            self.history_back.push(self.cwd.clone());
            self.history_fwd.clear();
            self.cwd = norm;
        }
        self.section = if self.cwd == TRASH_DIR { FilesSection::Trash } else { FilesSection::Directory };
        self.scroll_y = 0;
        self.selected.clear();
        self.renaming_idx = None;
        self.reload();
    }

    pub fn open_trash(&mut self) {
        let _ = std::fs::create_dir_all(TRASH_DIR);
        self.navigate(TRASH_DIR);
    }

    pub fn go_back(&mut self) {
        if let Some(prev) = self.history_back.pop() {
            self.history_fwd.push(self.cwd.clone());
            self.cwd = prev;
            self.section = if self.cwd == TRASH_DIR { FilesSection::Trash } else { FilesSection::Directory };
            self.reload();
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(next) = self.history_fwd.pop() {
            self.history_back.push(self.cwd.clone());
            self.cwd = next;
            self.section = if self.cwd == TRASH_DIR { FilesSection::Trash } else { FilesSection::Directory };
            self.reload();
        }
    }

    pub fn go_parent(&mut self) {
        if self.cwd == "/" { return; }
        let tr = self.cwd.trim_end_matches('/');
        let parent = match tr.rfind('/') {
            Some(0) | None => "/",
            Some(pos) => &tr[..pos],
        };
        let p = String::from(parent);
        self.navigate(&p);
    }

    pub(crate) fn refresh_inspector(&mut self) {
        if let Some(&idx) = self.selected.first() {
            if let Some(e) = self.entries.get(idx) {
                self.inspector_path = e.path.clone();
                self.inspector_info = Some(inspect_exofs_object(&e.path));
                return;
            }
        }
        self.inspector_path = self.cwd.clone();
        self.inspector_info = Some(inspect_exofs_object(&self.cwd));
    }

    pub(crate) fn filtered_indices(&self) -> Vec<usize> {
        let q = self.search.text.trim().to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (self.show_hidden || !e.name.starts_with('.'))
                    && (q.is_empty() || e.name.to_ascii_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

pub(crate) fn normalize_path(p: &str) -> String {
    let trimmed = p.trim();
    if trimmed.is_empty() || trimmed == "/" { String::from("/") }
    else if trimmed.starts_with('/') { String::from(trimmed.trim_end_matches('/')) }
    else { format!("/{}", trimmed.trim_end_matches('/')) }
}

pub(crate) fn join_path(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{}", name) } else { format!("{}/{}", dir.trim_end_matches('/'), name) }
}

pub(crate) fn is_read_only_zone(path: &str) -> bool {
    path == "/sbin" || path.starts_with("/sbin/")
        || path == "/bin" || path.starts_with("/bin/")
        || path == "/lib" || path.starts_with("/lib/")
}

pub(crate) fn format_bytes(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{}.{} MiB", b / (1024 * 1024), ((b % (1024 * 1024)) * 10) / (1024 * 1024))
    } else if b >= 1024 {
        format!("{} KiB", b / 1024)
    } else {
        format!("{} B", b)
    }
}
