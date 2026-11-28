//! Time-series anti-aliased area sparklines and horizontal bar charts (Dark & Light aware).

use alloc::vec::Vec;

use crate::canvas::Canvas;
use crate::color::{Color, Palette};
use crate::font::ExoFont;
use crate::widgets::Rect;

pub struct BarEntry<'a> {
    pub label: &'a str,
    pub value_text: &'a str,
    pub ratio: f32,
    pub color: Color,
}

pub fn draw_area_chart(
    cv: &mut Canvas<'_>,
    rect: Rect,
    samples: &[u32],
    max_hint: u32,
    color: Color,
    title: &str,
    right_label: &str,
    pal: &Palette,
    font_sm: &ExoFont<'_>,
    font_md: &ExoFont<'_>,
) {
    cv.fill_rounded_rect(rect, 12, pal.card_bg);
    cv.stroke_rounded_rect(rect, 12, pal.border_subtle);

    cv.draw_text(font_sm, rect.x + 14, rect.y + 10, title, pal.text_secondary);
    let rw = font_md.measure_width(right_label);
    cv.draw_text(font_md, rect.right() - rw - 14, rect.y + 8, right_label, color);

    let plot = Rect::new(rect.x + 14, rect.y + 34, rect.w - 28, (rect.h - 46).max(24));
    let grid_col = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x10)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x10)
    };
    for i in 0..=3 {
        let gy = plot.y + (plot.h * i) / 3;
        cv.fill_rect(Rect::new(plot.x, gy, plot.w, 1), grid_col);
    }

    if samples.len() < 2 {
        return;
    }
    let mut peak = max_hint.max(1);
    for &s in samples {
        if s > peak {
            peak = s;
        }
    }
    let n = samples.len();
    let step_x = (plot.w as f32) / ((n - 1) as f32);
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n + 2);
    pts.push((plot.x as f32, plot.bottom() as f32));
    for (i, &v) in samples.iter().enumerate() {
        let px = plot.x as f32 + (i as f32) * step_x;
        let ratio = (v as f32 / peak as f32).clamp(0.0, 1.0);
        let py = plot.bottom() as f32 - ratio * (plot.h as f32 - 4.0) - 2.0;
        pts.push((px, py));
    }
    pts.push((plot.right() as f32, plot.bottom() as f32));

    let prev = cv.push_clip(plot);
    cv.fill_polygon(&pts, color.with_alpha(if pal.is_dark { 0x36 } else { 0x28 }));
    for i in 1..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[i + 1];
        cv.draw_line_aa(x0, y0, x1, y1, 2.2, color);
    }
    if let Some(&(lx, ly)) = pts.get(n) {
        cv.fill_circle_aa(lx, ly, 4.0, color);
        cv.fill_circle_aa(lx, ly, 2.0, Color::WHITE);
    }
    cv.clip = prev;
}

pub fn draw_horizontal_bars(
    cv: &mut Canvas<'_>,
    rect: Rect,
    title: &str,
    entries: &[BarEntry<'_>],
    threshold_ratio: Option<f32>,
    pal: &Palette,
    font_sm: &ExoFont<'_>,
    font_md: &ExoFont<'_>,
) {
    cv.fill_rounded_rect(rect, 12, pal.card_bg);
    cv.stroke_rounded_rect(rect, 12, pal.border_subtle);
    cv.draw_text(font_md, rect.x + 14, rect.y + 10, title, pal.text_primary);

    if entries.is_empty() {
        return;
    }
    let start_y = rect.y + 38;
    let avail_h = (rect.h - 46).max(32);
    let row_h = (avail_h / entries.len() as i32).clamp(24, 38);
    let bar_x = rect.x + 14;
    let bar_w = (rect.w - 28).max(40);
    let track = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x12)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x10)
    };

    for (i, e) in entries.iter().enumerate() {
        let ry = start_y + (i as i32) * row_h;
        cv.draw_text_ellipsis(font_sm, bar_x, ry, e.label, bar_w - 110, pal.text_secondary);
        let vw = font_sm.measure_width(e.value_text);
        cv.draw_text(font_sm, bar_x + bar_w - vw, ry, e.value_text, pal.text_primary);

        let track_r = Rect::new(bar_x, ry + 16, bar_w, 7);
        cv.fill_rounded_rect(track_r, 3, track);
        let fw = ((bar_w as f32) * e.ratio.clamp(0.0, 1.0)) as i32;
        if fw > 0 {
            cv.fill_rounded_rect(Rect::new(bar_x, ry + 16, fw.max(6), 7), 3, e.color);
        }
    }

    if let Some(tr) = threshold_ratio {
        let tx = bar_x + ((bar_w as f32) * tr.clamp(0.0, 1.0)) as i32;
        cv.fill_rect(Rect::new(tx, start_y, 1, avail_h), pal.warning.with_alpha(0xBB));
    }
}
