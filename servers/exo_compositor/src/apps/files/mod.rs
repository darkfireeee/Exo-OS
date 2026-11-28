//! # Fichiers (ExoFS) — Explorateur modulaire conforme à la spécification §2.

mod events;
mod inspector;
mod nav;
mod ops;
mod view;

use crate::telemetry::ExoFsObjectInfo;
use exo_ui::TextField;

pub const USER_HOME: &str = "/home/user";
pub const USER_DESKTOP: &str = "/home/user/Desktop";
pub const USER_DOCS: &str = "/home/user/Documents";
pub const USER_NOTES: &str = "/home/user/Notes";
pub const USER_DOWNLOADS: &str = "/home/user/Downloads";
pub const USER_IMAGES: &str = "/home/user/Pictures";
pub const USER_PROJECTS: &str = "/home/user/Projects";
pub const TRASH_DIR: &str = "/tmp/.exo_trash";

/// Ensure the writable user home hierarchy exists outside `/` on boot & FS open.
pub fn ensure_user_home_dirs() -> &'static str {
    let dirs = [
        USER_HOME,
        USER_DESKTOP,
        USER_DOCS,
        USER_NOTES,
        USER_DOWNLOADS,
        USER_IMAGES,
        USER_PROJECTS,
        TRASH_DIR,
    ];
    let mut home_ok = false;
    for d in dirs {
        if std::fs::create_dir_all(d).is_ok() && d == USER_HOME {
            home_ok = true;
        }
    }
    if home_ok {
        let welcome = "/home/user/Documents/Welcome.txt";
        if std::fs::metadata(welcome).is_err() {
            let _ = std::fs::write(
                welcome,
                b"# Welcome to ExoOS 0.2.0\n\nYour personal folder /home/user is ready for reading and writing.\n",
            );
        }
        let notes = "/home/user/Notes/Notes.txt";
        if std::fs::metadata(notes).is_err() {
            let _ = std::fs::write(
                notes,
                b"// ExoOS Notes\nType your text here and press Ctrl+S to save.\n",
            );
        }
        USER_HOME
    } else {
        let _ = std::fs::create_dir_all("/tmp");
        "/tmp"
    }
}

pub fn default_writable_dir(preferred: &'static str) -> String {
    if std::fs::create_dir_all(preferred).is_ok() {
        String::from(preferred)
    } else {
        let _ = std::fs::create_dir_all("/tmp");
        String::from("/tmp")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode { Grid, List }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortBy { Name, Size, Kind }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilesSection { Directory, Trash, Snapshots }

#[derive(Clone, Debug)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub is_exec: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum FileUndoOp {
    Renamed { old_path: String, new_path: String },
    CreatedFile { path: String },
    CreatedDir { path: String },
    Trashed { orig_path: String, trash_path: String },
}

pub(crate) const SIDEBAR_PLACES: [(&str, &str, &str); 7] = [
    ("Accueil", "Home", USER_HOME),
    ("Bureau", "Desktop", USER_DESKTOP),
    ("Documents", "Documents", USER_DOCS),
    ("Notes", "Notes", USER_NOTES),
    ("Projets", "Projects", USER_PROJECTS),
    ("Temporaire", "Temp", "/tmp"),
    ("Racine /", "Root /", "/"),
];

pub struct FilesApp {
    pub cwd: String,
    pub section: FilesSection,
    pub entries: Vec<FsEntry>,
    pub selected: Vec<usize>,
    pub last_clicked: Option<usize>,
    pub view_mode: ViewMode,
    pub sort_by: SortBy,
    pub show_hidden: bool,
    pub scroll_y: i32,
    pub dragging_scrollbar: bool,
    pub history_back: Vec<String>,
    pub history_fwd: Vec<String>,
    pub search: TextField,
    pub search_focused: bool,
    pub renaming_idx: Option<usize>,
    pub rename_field: TextField,
    pub marquee_active: bool,
    pub marquee_start: (i32, i32),
    pub marquee_cur: (i32, i32),
    pub inspector_info: Option<ExoFsObjectInfo>,
    pub inspector_path: String,
    pub status_line: String,
    pub(crate) undo_stack: Vec<FileUndoOp>,
    pub(crate) last_click_tick: u64,
    pub(crate) last_click_item: Option<usize>,
}

pub(crate) use nav::{format_bytes, is_read_only_zone, join_path};
