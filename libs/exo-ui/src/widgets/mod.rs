//! Reusable geometry, interactive controls, option state badges, and floating context menus.

pub mod controls;
pub mod menu;

pub use controls::{
    compute_scrollbar, draw_badge, draw_button, draw_scrollbar, draw_slider, draw_state_pill,
    draw_toggle, scroll_from_drag_y, slider_ratio_from_x, OptionState, ScrollbarMetrics,
};
pub use menu::{ContextMenu, ContextMenuItem};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const ZERO: Self = Self { x: 0, y: 0, w: 0, h: 0 };

    #[inline(always)]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    #[inline(always)]
    pub const fn right(self) -> i32 {
        self.x + self.w
    }

    #[inline(always)]
    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }

    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[inline(always)]
    pub const fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    #[inline(always)]
    pub fn intersect(self, other: Self) -> Self {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Self::ZERO
        } else {
            Self::new(x0, y0, x1 - x0, y1 - y0)
        }
    }

    #[inline(always)]
    pub fn inset(self, dx: i32, dy: i32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            (self.w - dx * 2).max(0),
            (self.h - dy * 2).max(0),
        )
    }

    #[inline(always)]
    pub fn from_points(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let lx = x0.min(x1);
        let ly = y0.min(y1);
        Self::new(lx, ly, (x0 - x1).abs(), (y0 - y1).abs())
    }
}
