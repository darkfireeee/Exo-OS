//! Shared app identifiers, cross-app actions, context commands, and text field renderer.

use crate::assets::IconId;
use crate::bench::BenchCategory;
use crate::i18n::Lang;
use exo_ui::widgets::ContextMenuItem;
use exo_ui::{Canvas, Palette, Rect, TextField};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    Files,
    Terminal,
    Monitor,
    Settings,
    Editor,
    Bench,
    Games,
}

impl AppKind {
    pub const DOCK_APPS: [Self; 7] = [
        Self::Files,
        Self::Terminal,
        Self::Monitor,
        Self::Settings,
        Self::Editor,
        Self::Bench,
        Self::Games,
    ];

    pub const fn title(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Files, Lang::Fr) => "Fichiers",
            (Self::Files, Lang::En) => "Files",
            (Self::Terminal, _) => "Terminal",
            (Self::Monitor, Lang::Fr) => "Moniteur système",
            (Self::Monitor, Lang::En) => "System Monitor",
            (Self::Settings, Lang::Fr) => "Paramètres",
            (Self::Settings, Lang::En) => "Settings",
            (Self::Editor, Lang::Fr) => "Éditeur de texte",
            (Self::Editor, Lang::En) => "Text Editor",
            (Self::Bench, Lang::Fr) => "Performances (ExoBench)",
            (Self::Bench, Lang::En) => "Performance (ExoBench)",
            (Self::Games, Lang::Fr) => "Jeux",
            (Self::Games, Lang::En) => "Games",
        }
    }

    pub const fn short_label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Files, Lang::Fr) => "Fichiers",
            (Self::Files, Lang::En) => "Files",
            (Self::Terminal, _) => "Terminal",
            (Self::Monitor, Lang::Fr) => "Moniteur",
            (Self::Monitor, Lang::En) => "Monitor",
            (Self::Settings, Lang::Fr) => "Paramètres",
            (Self::Settings, Lang::En) => "Settings",
            (Self::Editor, Lang::Fr) => "Éditeur",
            (Self::Editor, Lang::En) => "Editor",
            (Self::Bench, _) => "ExoBench",
            (Self::Games, Lang::Fr) => "Jeux",
            (Self::Games, Lang::En) => "Games",
        }
    }

    pub const fn icon(self) -> IconId {
        match self {
            Self::Files => IconId::Files,
            Self::Terminal => IconId::Terminal,
            Self::Monitor => IconId::Monitor,
            Self::Settings => IconId::Settings,
            Self::Editor => IconId::Editor,
            Self::Bench => IconId::Bench,
            Self::Games => IconId::Games,
        }
    }

    pub fn default_size(self, screen_w: i32, screen_h: i32) -> (i32, i32) {
        let max_w = (screen_w - 60).max(480);
        let max_h = (screen_h - 124).max(320);
        let (w, h) = match self {
            Self::Files => (980, 610),
            Self::Monitor => (980, 620),
            Self::Settings => (940, 600),
            Self::Bench => (920, 600),
            Self::Terminal => (760, 490),
            Self::Editor => (760, 530),
            Self::Games => (640, 520),
        };
        (w.min(max_w), h.min(max_h))
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum AppAction {
    None,
    Redraw,
    OpenTerminalAt(String),
    OpenFileInEditor(String),
    OpenApp(AppKind),
    OpenFilesAt(String),
    OpenFilesNewWindow(String),
    OpenSettingsCategory(usize),
    RequestFilePicker { app_name: String },
    RequestAdminElevation { reason: String },
    ShowContextMenu { x: i32, y: i32, items: Vec<ContextMenuItem> },
    TriggerPhoenixSwitch,
    CreateSnapshot(String),
    RunBenchAll,
    RunBenchModule(BenchCategory),
    ExportBenchReport,
    CopyText(String),
    TerminatePid(u32),
    LockScreen,
    StatusToast(String),
    Reboot,
    Shutdown,
}

pub mod ctx_cmd {
    pub const SELECT_ALL: u32 = 1001;
    pub const COPY: u32 = 1002;
    pub const CUT: u32 = 1003;
    pub const PASTE: u32 = 1004;
    pub const UNDO: u32 = 1005;
    pub const REDO: u32 = 1006;

    pub const FILES_OPEN: u32 = 2001;
    pub const FILES_NEW_DIR: u32 = 2002;
    pub const FILES_NEW_FILE: u32 = 2003;
    pub const FILES_RENAME: u32 = 2004;
    pub const FILES_TRASH: u32 = 2005;
    pub const FILES_OPEN_TERM: u32 = 2006;
    pub const FILES_SNAPSHOT: u32 = 2007;
    pub const FILES_REFRESH: u32 = 2008;
    pub const FILES_RESTORE: u32 = 2009;
    pub const FILES_DELETE_PERM: u32 = 2010;
    pub const FILES_OPEN_NEW_WIN: u32 = 2011;

    pub const TERM_CLEAR: u32 = 3001;
    pub const EDITOR_SAVE: u32 = 4001;
    pub const EDITOR_OPEN: u32 = 4002;

    pub const DESK_OPEN_FILES: u32 = 5001;
    pub const DESK_OPEN_TERM: u32 = 5002;
    pub const DESK_OPEN_MONITOR: u32 = 5003;
    pub const DESK_OPEN_SETTINGS: u32 = 5004;
    pub const DESK_OPEN_BENCH: u32 = 5005;
    pub const DESK_TOGGLE_THEME: u32 = 5006;
    pub const DESK_NEXT_WALLPAPER: u32 = 5007;
}

pub fn draw_textfield(
    cv: &mut Canvas<'_>,
    rect: Rect,
    tf: &TextField,
    placeholder: &str,
    focused: bool,
    pal: &Palette,
    font: &exo_ui::ExoFont<'_>,
) {
    let prev = cv.push_clip(rect);
    let ty = rect.y + (rect.h - font.line_height) / 2;
    if tf.text.is_empty() && !placeholder.is_empty() {
        cv.draw_text(font, rect.x, ty, placeholder, pal.text_muted);
    } else {
        if tf.has_selection() {
            let (s, e) = tf.selection_range();
            let sx = rect.x + font.x_of_byte_index(&tf.text, s);
            let ex = rect.x + font.x_of_byte_index(&tf.text, e);
            cv.fill_rounded_rect(Rect::new(sx, ty, (ex - sx).max(3), font.line_height), 3, exo_ui::Color::rgba(0x3B, 0x82, 0xF6, 0x7A));
        }
        cv.draw_text(font, rect.x, ty, &tf.text, pal.text_primary);
    }
    if focused {
        let cx = rect.x + font.x_of_byte_index(&tf.text, tf.cursor);
        cv.fill_rect(Rect::new(cx, ty + 1, 2, (font.line_height - 2).max(10)), pal.accent);
    }
    cv.clip = prev;
}
