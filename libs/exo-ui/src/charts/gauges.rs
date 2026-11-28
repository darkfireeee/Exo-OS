//! Radial donut gauge component (palette-aware Dark & Light).

use crate::canvas::Canvas;
use crate::color::{Color, Palette};
use crate::font::ExoFont;
use crate::widgets::Rect;

pub fn draw_radial_gauge(
    cv: &mut Canvas<'_>,
    rect: Rect,
    percent: f32,
    color: Color,
    center_text: &str,
    title: &str,
    subtitle: &str,
    pal: &Palette,
    font_sm: &ExoFont<'_>,
    font_hero: &ExoFont<'_>,
) {
    cv.fill_rounded_rect(rect, 12, pal.card_bg);
    cv.stroke_rounded_rect(rect, 12, pal.border_subtle);

    let cx = rect.x as f32 + rect.w as f32 * 0.5;
    let cy = rect.y as f32 + (rect.h as f32 * 0.42).max(40.0);
    let rad = ((rect.w.min(rect.h) as f32) * 0.28).clamp(24.0, 44.0);
    let track = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x16)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x14)
    };

    cv.stroke_arc_aa(cx, cy, rad, 7.0, 0.0, 360.0, track);
    let sweep = (percent.clamp(0.0, 100.0) / 100.0) * 359.8;
    if sweep > 0.5 {
        cv.stroke_arc_aa(cx, cy, rad, 7.0, 0.0, sweep, color);
    }

    let cw = font_hero.measure_width(center_text);
    cv.draw_text(
        font_hero,
        (cx as i32) - cw / 2,
        (cy as i32) - font_hero.line_height / 2,
        center_text,
        pal.text_primary,
    );

    let tw = font_sm.measure_width(title);
    cv.draw_text(
        font_sm,
        rect.x + (rect.w - tw) / 2,
        rect.bottom() - 36,
        title,
        pal.text_primary,
    );
    let sw = font_sm.measure_width(subtitle);
    cv.draw_text(
        font_sm,
        rect.x + (rect.w - sw) / 2,
        rect.bottom() - 19,
        subtitle,
        pal.text_muted,
    );
}
