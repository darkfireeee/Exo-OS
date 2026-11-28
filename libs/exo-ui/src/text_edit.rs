//! Global clipboard, mouse/keyboard text selection, and undo/redo history (`Ctrl+A/C/X/V/Z/Y`).

use alloc::string::String;
use alloc::vec::Vec;

/// System-wide Strata clipboard supporting text and ExoFS file copy/cut payloads.
#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    pub text: String,
    pub file_paths: Vec<String>,
    pub file_cut: bool,
}

impl Clipboard {
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            file_paths: Vec::new(),
            file_cut: false,
        }
    }

    pub fn set_text(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }

    pub fn get_text(&self) -> &str {
        &self.text
    }

    pub fn set_files(&mut self, paths: Vec<String>, cut: bool) {
        self.file_paths = paths;
        self.file_cut = cut;
    }

    pub fn clear_files(&mut self) {
        self.file_paths.clear();
        self.file_cut = false;
    }
}

#[derive(Clone, Debug)]
struct TextSnapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

/// Interactive UTF-8 text field with mouse drag selection, double-click word selection,
/// `Ctrl+A` / `Ctrl+C` / `Ctrl+X` / `Ctrl+V` / `Ctrl+Z` / `Ctrl+Y`, and undo/redo stack.
#[derive(Clone, Debug)]
pub struct TextField {
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
    pub dragging: bool,
    undo_stack: Vec<TextSnapshot>,
    redo_stack: Vec<TextSnapshot>,
}

impl Default for TextField {
    fn default() -> Self {
        Self::new()
    }
}

impl TextField {
    const MAX_UNDO: usize = 64;
    const MAX_TEXT_BYTES: usize = 256 * 1024;
    const MAX_UNDO_BYTES: usize = 512 * 1024;

    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: 0,
            dragging: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn with_text(s: &str) -> Self {
        let mut tf = Self::new();
        tf.text.push_str(s);
        tf.cursor = tf.text.len();
        tf.anchor = tf.cursor;
        tf
    }

    pub fn set_text(&mut self, s: &str) {
        self.push_undo();
        self.text.clear();
        self.text.push_str(s);
        self.cursor = self.text.len();
        self.anchor = self.cursor;
    }

    fn push_undo(&mut self) {
        // Skip cloning huge texts repeatedly when undo memory budget is already saturated
        if self.text.len() > Self::MAX_UNDO_BYTES / 2 {
            self.undo_stack.clear();
        } else {
            let mut total_bytes: usize = self.undo_stack.iter().map(|u| u.text.len()).sum();
            while (self.undo_stack.len() >= Self::MAX_UNDO
                || total_bytes.saturating_add(self.text.len()) > Self::MAX_UNDO_BYTES)
                && !self.undo_stack.is_empty()
            {
                let removed = self.undo_stack.remove(0);
                total_bytes = total_bytes.saturating_sub(removed.text.len());
            }
        }
        self.undo_stack.push(TextSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.redo_stack.clear();
    }

    #[inline]
    pub fn selection_range(&self) -> (usize, usize) {
        let a = self.anchor.min(self.text.len());
        let b = self.cursor.min(self.text.len());
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    }

    #[inline]
    pub fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    pub fn selected_text(&self) -> &str {
        let (s, e) = self.selection_range();
        &self.text[s..e]
    }

    /// `Ctrl+A` / Mouse Context Menu "Tout sélectionner"
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    /// Double-click word selection around `byte_idx`.
    pub fn select_word_at(&mut self, byte_idx: usize) {
        let idx = self.clamp_char_boundary(byte_idx);
        let bytes = self.text.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.';
        let mut s = idx;
        while s > 0 && is_word(bytes[s - 1]) {
            s -= 1;
        }
        let mut e = idx;
        while e < bytes.len() && is_word(bytes[e]) {
            e += 1;
        }
        self.anchor = s;
        self.cursor = e;
    }

    /// Mouse press inside text field at resolved byte index.
    pub fn mouse_press(&mut self, byte_idx: usize, shift_held: bool) {
        let idx = self.clamp_char_boundary(byte_idx);
        self.cursor = idx;
        if !shift_held {
            self.anchor = idx;
        }
        self.dragging = true;
    }

    /// Mouse drag move inside text field.
    pub fn mouse_drag(&mut self, byte_idx: usize) {
        if self.dragging {
            self.cursor = self.clamp_char_boundary(byte_idx);
        }
    }

    pub fn mouse_release(&mut self) {
        self.dragging = false;
    }

    /// `Ctrl+C` / Mouse "Copier"
    pub fn copy(&self, clip: &mut Clipboard) -> bool {
        if self.has_selection() {
            clip.set_text(self.selected_text());
            true
        } else if !self.text.is_empty() {
            clip.set_text(&self.text);
            true
        } else {
            false
        }
    }

    /// `Ctrl+X` / Mouse "Couper"
    pub fn cut(&mut self, clip: &mut Clipboard) -> bool {
        if !self.has_selection() {
            return false;
        }
        clip.set_text(self.selected_text());
        self.push_undo();
        self.delete_selection_inner();
        true
    }

    /// `Ctrl+V` / Mouse "Coller"
    pub fn paste(&mut self, clip: &Clipboard) -> bool {
        if clip.text.is_empty() {
            return false;
        }
        self.insert_str(&clip.text);
        true
    }

    /// `Ctrl+Z` / Mouse "Annuler"
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(TextSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.text = prev.text;
        self.cursor = prev.cursor.min(self.text.len());
        self.anchor = prev.anchor.min(self.text.len());
        true
    }

    /// `Ctrl+Y` / `Ctrl+Shift+Z` / Mouse "Rétablir"
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(TextSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.text = next.text;
        self.cursor = next.cursor.min(self.text.len());
        self.anchor = next.anchor.min(self.text.len());
        true
    }

    pub fn insert_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.insert_str(s);
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let (sel_s, sel_e) = self.selection_range();
        let resulting_len = self.text.len().saturating_sub(sel_e - sel_s).saturating_add(s.len());
        if resulting_len > Self::MAX_TEXT_BYTES {
            let room = Self::MAX_TEXT_BYTES.saturating_sub(self.text.len().saturating_sub(sel_e - sel_s));
            if room == 0 {
                return;
            }
            let mut cut = room.min(s.len());
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            if cut == 0 {
                return;
            }
            self.push_undo();
            self.delete_selection_inner();
            let pos = self.clamp_char_boundary(self.cursor);
            self.text.insert_str(pos, &s[..cut]);
            self.cursor = pos + cut;
            self.anchor = self.cursor;
            return;
        }
        self.push_undo();
        self.delete_selection_inner();
        let pos = self.clamp_char_boundary(self.cursor);
        self.text.insert_str(pos, s);
        self.cursor = pos + s.len();
        self.anchor = self.cursor;
    }

    pub fn backspace(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection_inner();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let prev = self.prev_char_boundary(self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
        self.anchor = prev;
    }

    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection_inner();
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        self.push_undo();
        let next = self.next_char_boundary(self.cursor);
        self.text.drain(self.cursor..next);
        self.anchor = self.cursor;
    }

    pub fn move_left(&mut self, shift: bool) {
        if !shift && self.has_selection() {
            let (s, _) = self.selection_range();
            self.cursor = s;
            self.anchor = s;
            return;
        }
        self.cursor = self.prev_char_boundary(self.cursor);
        if !shift {
            self.anchor = self.cursor;
        }
    }

    pub fn move_right(&mut self, shift: bool) {
        if !shift && self.has_selection() {
            let (_, e) = self.selection_range();
            self.cursor = e;
            self.anchor = e;
            return;
        }
        self.cursor = self.next_char_boundary(self.cursor);
        if !shift {
            self.anchor = self.cursor;
        }
    }

    pub fn move_home(&mut self, shift: bool) {
        self.cursor = 0;
        if !shift {
            self.anchor = 0;
        }
    }

    pub fn move_end(&mut self, shift: bool) {
        self.cursor = self.text.len();
        if !shift {
            self.anchor = self.cursor;
        }
    }

    fn delete_selection_inner(&mut self) {
        if !self.has_selection() {
            return;
        }
        let (s, e) = self.selection_range();
        self.text.drain(s..e);
        self.cursor = s;
        self.anchor = s;
    }

    fn clamp_char_boundary(&self, mut idx: usize) -> usize {
        idx = idx.min(self.text.len());
        while idx > 0 && !self.text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn prev_char_boundary(&self, mut idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        idx = idx.min(self.text.len()) - 1;
        while idx > 0 && !self.text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn next_char_boundary(&self, mut idx: usize) -> usize {
        if idx >= self.text.len() {
            return self.text.len();
        }
        idx += 1;
        while idx < self.text.len() && !self.text.is_char_boundary(idx) {
            idx += 1;
        }
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_a_c_v_x_z_y_workflow() {
        let mut clip = Clipboard::new();
        let mut tf = TextField::with_text("Exo-OS Strata");
        tf.select_all();
        assert_eq!(tf.selected_text(), "Exo-OS Strata");
        assert!(tf.copy(&mut clip));
        assert_eq!(clip.get_text(), "Exo-OS Strata");

        tf.mouse_press(0, false);
        tf.mouse_drag(6);
        tf.mouse_release();
        assert!(tf.cut(&mut clip));
        assert_eq!(clip.get_text(), "Exo-OS");
        assert_eq!(tf.text, " Strata");

        assert!(tf.undo());
        assert_eq!(tf.text, "Exo-OS Strata");
        assert!(tf.redo());
        assert_eq!(tf.text, " Strata");

        tf.move_end(false);
        assert!(tf.paste(&clip));
        assert_eq!(tf.text, " StrataExo-OS");
    }
}
