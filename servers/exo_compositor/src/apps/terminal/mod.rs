//! # Terminal (exosh) — Multi-tab interactive shell on demand (ExoOS UI Spec §4.1).

mod shell_exec;
mod view;

use exo_ui::{Color, TextField};

#[derive(Clone, Debug)]
pub(crate) struct TermLine {
    pub text: String,
    pub color: Color,
}

pub(crate) struct TermTab {
    pub title: String,
    pub cwd: String,
    pub lines: Vec<TermLine>,
    pub input: TextField,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub scroll_offset: usize,
    pub dragging_scrollbar: bool,
    pub sel_anchor: Option<(usize, usize)>,
    pub sel_cursor: Option<(usize, usize)>,
    pub selecting_output: bool,
}

pub const USER_HOME_DIR: &str = "/home/user";

impl TermTab {
    pub(crate) fn new(idx: usize, initial_cwd: &str) -> Self {
        let cwd = if initial_cwd.is_empty() {
            let _ = std::fs::create_dir_all(USER_HOME_DIR);
            String::from(USER_HOME_DIR)
        } else {
            String::from(initial_cwd)
        };
        let mut t = Self {
            title: format!("exosh #{}", idx),
            cwd,
            lines: Vec::new(),
            input: TextField::new(),
            history: Vec::new(),
            history_idx: None,
            scroll_offset: 0,
            dragging_scrollbar: false,
            sel_anchor: None,
            sel_cursor: None,
            selecting_output: false,
        };
        t.push_line("ExoOS Terminal (exosh) — type 'help', 'ping 1.1.1.1', 'nettest' or 'exo bench'", Color::rgb(0x10, 0xB9, 0x81));
        t.push_line("", Color::rgb(0xF3, 0xF4, 0xF6));
        t
    }

    pub(crate) fn prompt_text(&self) -> String {
        if self.cwd == USER_HOME_DIR {
            String::from("user@exo:~ $ ")
        } else if let Some(rest) = self.cwd.strip_prefix("/home/user/") {
            format!("user@exo:~/{} $ ", rest)
        } else {
            format!("user@exo:{} $ ", self.cwd)
        }
    }

    pub(crate) fn push_line(&mut self, text: impl Into<String>, color: Color) {
        self.lines.push(TermLine { text: text.into(), color });
        if self.lines.len() > 400 { self.lines.drain(..50); }
        self.scroll_offset = 0;
    }

    pub(crate) fn visible_window(&self, out_h: i32, line_h: i32) -> (usize, usize) {
        let cap = ((out_h - 14) / line_h.max(12)).max(1) as usize;
        let end = self.lines.len().saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(cap);
        (start, end)
    }

    pub(crate) fn normalized_sel(&self) -> Option<((usize, usize), (usize, usize))> {
        let (a, b) = (self.sel_anchor?, self.sel_cursor?);
        if a == b { None } else if a <= b { Some((a, b)) } else { Some((b, a)) }
    }

    pub(crate) fn extract_sel(&self) -> Option<String> {
        let ((l0, b0), (l1, b1)) = self.normalized_sel()?;
        let mut out = String::new();
        for li in l0..=l1.min(self.lines.len().saturating_sub(1)) {
            let s = &self.lines[li].text;
            let st = if li == l0 { clamp_boundary(s, b0) } else { 0 };
            let en = if li == l1 { clamp_boundary(s, b1) } else { s.len() };
            if st <= en { out.push_str(&s[st..en]); }
            if li < l1 { out.push('\n'); }
        }
        if out.is_empty() { None } else { Some(out) }
    }
}

pub struct TerminalApp {
    pub(crate) tabs: Vec<TermTab>,
    pub(crate) active: usize,
    pub(crate) next_tab_num: usize,
}

impl TerminalApp {
    pub fn new(initial_cwd: &str) -> Self {
        Self { tabs: vec![TermTab::new(1, initial_cwd)], active: 0, next_tab_num: 2 }
    }
    pub(crate) fn cur(&self) -> &TermTab { &self.tabs[self.active] }
    pub(crate) fn cur_mut(&mut self) -> &mut TermTab { &mut self.tabs[self.active] }
}

pub(crate) fn clamp_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) { idx -= 1; }
    idx
}
