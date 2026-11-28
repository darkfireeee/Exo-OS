//! Clean interactive controls (sliders, toggles, buttons, badges, and spec §0 OptionState pills).

use crate::canvas::Canvas;
use crate::color::{Color, Palette};
use crate::font::ExoFont;
use crate::widgets::Rect;

/// Canonical setting/feature state mandated by ExoOS UI Spec §0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionState {
    Applied,
    PendingReboot,
    PolicyDenied,
    Unavailable,
    AdminLocked,
    Error,
}

impl OptionState {
    pub fn label(self, en: bool) -> &'static str {
        match (self, en) {
            (Self::Applied, false) => "Appliqué",
            (Self::Applied, true) => "Applied",
            (Self::PendingReboot, false) => "Redémarrage requis",
            (Self::PendingReboot, true) => "Reboot required",
            (Self::PolicyDenied, false) => "Refusé par politique",
            (Self::PolicyDenied, true) => "Denied by policy",
            (Self::Unavailable, false) => "Matériel absent",
            (Self::Unavailable, true) => "Unavailable",
            (Self::AdminLocked, false) => "Admin (ExoAuth)",
            (Self::AdminLocked, true) => "Admin (ExoAuth)",
            (Self::Error, false) => "Erreur",
            (Self::Error, true) => "Error",
        }
    }

    pub const fn color(self, pal: &Palette) -> Color {
        match self {
            Self::Applied => pal.success,
            Self::PendingReboot => pal.warning,
            Self::PolicyDenied => pal.danger,
            Self::Unavailable => pal.text_muted,
            Self::AdminLocked => pal.warning,
            Self::Error => pal.danger,
        }
    }
}

pub fn draw_slider(
    cv: &mut Canvas<'_>,
    rect: Rect,
    ratio: f32,
    accent: Color,
    pal: &Palette,
    hovered: bool,
) {
    let r = ratio.clamp(0.0, 1.0);
    let cy = rect.y + rect.h / 2;
    let track = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x20)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x1A)
    };
    cv.fill_rounded_rect(Rect::new(rect.x, cy - 3, rect.w, 6), 3, track);
    let fill_w = ((rect.w as f32) * r) as i32;
    if fill_w > 0 {
        cv.fill_rounded_rect(Rect::new(rect.x, cy - 3, fill_w.max(6), 6), 3, accent);
    }
    let kx = rect.x as f32 + (rect.w as f32) * r;
    let ky = cy as f32;
    if hovered {
        cv.fill_circle_aa(kx, ky, 12.0, accent.with_alpha(0x38));
    }
    cv.fill_circle_aa(kx, ky, 8.5, Color::WHITE);
    cv.fill_circle_aa(kx, ky, 4.5, accent);
}

#[inline]
pub fn slider_ratio_from_x(rect: Rect, mx: i32) -> f32 {
    if rect.w <= 1 {
        return 0.0;
    }
    ((mx - rect.x) as f32 / rect.w as f32).clamp(0.0, 1.0)
}

pub fn draw_toggle(cv: &mut Canvas<'_>, rect: Rect, enabled: bool, accent: Color, pal: &Palette) {
    let rad = rect.h / 2;
    let off_bg = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x26)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x24)
    };
    cv.fill_rounded_rect(rect, rad, if enabled { accent } else { off_bg });
    let kx = if enabled {
        rect.right() - rad
    } else {
        rect.x + rad
    };
    cv.fill_circle_aa(kx as f32, (rect.y + rad) as f32, (rad - 3).max(4) as f32, Color::WHITE);
}

pub fn draw_badge(
    cv: &mut Canvas<'_>,
    x: i32,
    y: i32,
    text: &str,
    tint: Color,
    font: &ExoFont<'_>,
) -> i32 {
    let tw = font.measure_width(text);
    let w = tw + 16;
    let h = font.line_height + 6;
    let r = Rect::new(x, y, w, h);
    cv.fill_rounded_rect(r, h / 2, tint.with_alpha(0x22));
    cv.stroke_rounded_rect(r, h / 2, tint.with_alpha(0x70));
    cv.draw_text(font, x + 8, y + 3, text, tint);
    w
}

pub fn draw_state_pill(
    cv: &mut Canvas<'_>,
    x: i32,
    y: i32,
    state: OptionState,
    en: bool,
    pal: &Palette,
    font: &ExoFont<'_>,
) -> i32 {
    draw_badge(cv, x, y, state.label(en), state.color(pal), font)
}

pub fn draw_button(
    cv: &mut Canvas<'_>,
    rect: Rect,
    label: &str,
    primary: bool,
    hovered: bool,
    pal: &Palette,
    font: &ExoFont<'_>,
) {
    let (bg, border, fg) = if primary {
        let fill = if hovered { pal.accent } else { pal.accent.with_alpha(0xE0) };
        (fill, pal.accent, Color::WHITE)
    } else {
        let fill = if hovered { pal.hover_bg } else { pal.elevated_bg };
        (fill, pal.border_subtle, pal.text_primary)
    };
    cv.fill_rounded_rect(rect, 8, bg);
    cv.stroke_rounded_rect(rect, 8, border);
    let tw = font.measure_width(label);
    cv.draw_text(
        font,
        rect.x + (rect.w - tw) / 2,
        rect.y + (rect.h - font.line_height) / 2,
        label,
        fg,
    );
}

/// Geometry & scroll bounds for a modern slim overlay vertical scrollbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollbarMetrics {
    pub track: Rect,
    pub thumb: Rect,
    pub max_scroll: i32,
}

/// Compute vertical scrollbar geometry inside `viewport`. Returns `None` if all content fits.
pub fn compute_scrollbar(viewport: Rect, content_h: i32, scroll_y: i32) -> Option<ScrollbarMetrics> {
    if viewport.h <= 24 || content_h <= viewport.h {
        return None;
    }
    let max_scroll = (content_h - viewport.h).max(1);
    let clamped = scroll_y.clamp(0, max_scroll);
    let track = Rect::new(viewport.right() - 10, viewport.y + 4, 8, (viewport.h - 8).max(16));
    let thumb_h = (((viewport.h as i64 * track.h as i64) / content_h.max(1) as i64) as i32)
        .clamp(28, track.h);
    let travel = (track.h - thumb_h).max(0);
    let thumb_y = track.y + ((clamped as i64 * travel as i64) / max_scroll as i64) as i32;
    let thumb = Rect::new(track.x + 1, thumb_y, (track.w - 2).max(4), thumb_h);
    Some(ScrollbarMetrics {
        track,
        thumb,
        max_scroll,
    })
}

/// Render a clean rounded scrollbar pill with hover/active highlight.
pub fn draw_scrollbar(
    cv: &mut Canvas<'_>,
    metrics: ScrollbarMetrics,
    hovered: bool,
    dragging: bool,
    pal: &Palette,
) {
    let track_bg = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x0C)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x0C)
    };
    let thumb_col = if dragging {
        pal.accent
    } else if hovered {
        pal.accent.with_alpha(0xC8)
    } else if pal.is_dark {
        Color::rgba(0xE2, 0xE8, 0xF0, 0x58)
    } else {
        Color::rgba(0x33, 0x41, 0x55, 0x62)
    };
    cv.fill_rounded_rect(metrics.track, 4, track_bg);
    cv.fill_rounded_rect(metrics.thumb, 3, thumb_col);
}

/// Map a vertical pointer Y coordinate along the scrollbar track to a clamped `scroll_y` offset.
#[inline]
pub fn scroll_from_drag_y(metrics: ScrollbarMetrics, my: i32) -> i32 {
    let travel = (metrics.track.h - metrics.thumb.h).max(1);
    let rel = (my - metrics.track.y - metrics.thumb.h / 2).clamp(0, travel);
    ((rel as i64 * metrics.max_scroll as i64) / travel as i64) as i32
}
