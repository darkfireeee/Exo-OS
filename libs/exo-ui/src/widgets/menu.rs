//! Floating right-click context menu (palette-aware Dark & Light).

use alloc::string::String;
use alloc::vec::Vec;

use crate::canvas::Canvas;
use crate::color::Palette;
use crate::font::ExoFont;
use crate::widgets::Rect;

#[derive(Clone, Debug)]
pub struct ContextMenuItem {
    pub id: u32,
    pub label: String,
    pub shortcut: String,
    pub enabled: bool,
    pub separator_before: bool,
    pub danger: bool,
}

impl ContextMenuItem {
    pub fn new(id: u32, label: &str, shortcut: &str, enabled: bool) -> Self {
        Self {
            id,
            label: String::from(label),
            shortcut: String::from(shortcut),
            enabled,
            separator_before: false,
            danger: false,
        }
    }

    pub fn with_sep(mut self) -> Self {
        self.separator_before = true;
        self
    }

    pub fn with_danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct ContextMenu {
    pub open: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub items: Vec<ContextMenuItem>,
}

impl ContextMenu {
    pub const ITEM_H: i32 = 30;
    pub const PAD_Y: i32 = 6;

    pub fn new() -> Self {
        Self {
            open: false,
            x: 0,
            y: 0,
            width: 248,
            items: Vec::new(),
        }
    }

    pub fn show(&mut self, x: i32, y: i32, items: Vec<ContextMenuItem>, screen_w: i32, screen_h: i32) {
        self.items = items;
        let h = self.height();
        self.x = x.clamp(8, (screen_w - self.width - 8).max(8));
        self.y = y.clamp(8, (screen_h - h - 8).max(8));
        self.open = !self.items.is_empty();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.items.clear();
    }

    pub fn height(&self) -> i32 {
        let mut h = Self::PAD_Y * 2;
        for it in &self.items {
            if it.separator_before {
                h += 9;
            }
            h += Self::ITEM_H;
        }
        h
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height())
    }

    pub fn hit_test(&self, mx: i32, my: i32) -> Option<u32> {
        if !self.open || !self.bounds().contains(mx, my) {
            return None;
        }
        let mut cy = self.y + Self::PAD_Y;
        for it in &self.items {
            if it.separator_before {
                cy += 9;
            }
            let r = Rect::new(self.x + 4, cy, self.width - 8, Self::ITEM_H);
            if r.contains(mx, my) {
                return if it.enabled { Some(it.id) } else { None };
            }
            cy += Self::ITEM_H;
        }
        None
    }

    pub fn draw(
        &self,
        cv: &mut Canvas<'_>,
        mx: i32,
        my: i32,
        pal: &Palette,
        blur: bool,
        font_sm: &ExoFont<'_>,
        font_md: &ExoFont<'_>,
    ) {
        if !self.open {
            return;
        }
        let b = self.bounds();
        cv.draw_panel(b, 12, pal.menu_bg, pal.border_strong, blur, true);
        let mut cy = self.y + Self::PAD_Y;
        for it in &self.items {
            if it.separator_before {
                cv.fill_rect(
                    Rect::new(self.x + 12, cy + 4, self.width - 24, 1),
                    pal.border_subtle,
                );
                cy += 9;
            }
            let row = Rect::new(self.x + 5, cy, self.width - 10, Self::ITEM_H);
            let hovered = it.enabled && row.contains(mx, my);
            if hovered {
                let hl = if it.danger {
                    pal.danger.with_alpha(0x28)
                } else {
                    pal.accent.with_alpha(0x2A)
                };
                cv.fill_rounded_rect(row, 7, hl);
            }
            let fg = if !it.enabled {
                pal.text_muted
            } else if it.danger {
                pal.danger
            } else {
                pal.text_primary
            };
            cv.draw_text(font_md, row.x + 10, row.y + 7, &it.label, fg);
            if !it.shortcut.is_empty() {
                let sw = font_sm.measure_width(&it.shortcut);
                cv.draw_text(
                    font_sm,
                    row.right() - sw - 10,
                    row.y + 8,
                    &it.shortcut,
                    pal.text_muted,
                );
            }
            cy += Self::ITEM_H;
        }
    }
}
