//! Hexagonal 6-axis Radar chart and clean 3-module architecture diagram (Dark & Light aware).

use alloc::vec::Vec;

use crate::canvas::{sin_cos_f32, Canvas};
use crate::color::{Color, Palette};
use crate::font::ExoFont;
use crate::widgets::Rect;

pub fn draw_hexagon_radar(
    cv: &mut Canvas<'_>,
    rect: Rect,
    title: &str,
    axes: &[(&str, f32); 6],
    accent: Color,
    pal: &Palette,
    font_sm: &ExoFont<'_>,
    font_md: &ExoFont<'_>,
) {
    cv.fill_rounded_rect(rect, 12, pal.card_bg);
    cv.stroke_rounded_rect(rect, 12, pal.border_subtle);
    cv.draw_text(font_md, rect.x + 14, rect.y + 10, title, pal.text_primary);

    let cx = rect.x as f32 + rect.w as f32 * 0.5;
    let cy = rect.y as f32 + rect.h as f32 * 0.55;
    let max_r = ((rect.w.min(rect.h) as f32) * 0.30).clamp(38.0, 86.0);
    let ring_col = if pal.is_dark {
        Color::rgba(0xFF, 0xFF, 0xFF, 0x1A)
    } else {
        Color::rgba(0x11, 0x18, 0x27, 0x18)
    };

    for ring in 1..=4 {
        let rr = max_r * (ring as f32 / 4.0);
        let mut prev = (0.0f32, 0.0f32);
        for k in 0..=6 {
            let ang = (k as f32) * (core::f32::consts::PI / 3.0) - core::f32::consts::FRAC_PI_2;
            let (s, c) = sin_cos_f32(ang);
            let pt = (cx + c * rr, cy + s * rr);
            if k > 0 {
                cv.draw_line_aa(prev.0, prev.1, pt.0, pt.1, 1.0, ring_col);
            }
            prev = pt;
        }
    }

    let mut poly: Vec<(f32, f32)> = Vec::with_capacity(6);
    for (k, &(label, score)) in axes.iter().enumerate() {
        let ang = (k as f32) * (core::f32::consts::PI / 3.0) - core::f32::consts::FRAC_PI_2;
        let (s, c) = sin_cos_f32(ang);
        cv.draw_line_aa(cx, cy, cx + c * max_r, cy + s * max_r, 1.0, ring_col);
        let val_r = max_r * score.clamp(0.08, 1.0);
        poly.push((cx + c * val_r, cy + s * val_r));

        let lw = font_sm.measure_width(label);
        let lx = (cx + c * (max_r + 28.0)) as i32 - lw / 2;
        let ly = (cy + s * (max_r + 20.0)) as i32 - font_sm.line_height / 2;
        cv.draw_text(font_sm, lx, ly, label, pal.text_secondary);
    }

    cv.fill_polygon(&poly, accent.with_alpha(if pal.is_dark { 0x3A } else { 0x2C }));
    for i in 0..6 {
        let a = poly[i];
        let b = poly[(i + 1) % 6];
        cv.draw_line_aa(a.0, a.1, b.0, b.1, 2.2, accent);
    }
    for &(px, py) in &poly {
        cv.fill_circle_aa(px, py, 4.0, accent);
        cv.fill_circle_aa(px, py, 2.0, Color::WHITE);
    }
}

/// Clean 3-tier system health diagram (Noyau, Services, Interface).
pub fn draw_module_map(
    cv: &mut Canvas<'_>,
    rect: Rect,
    title: &str,
    nodes: [(&str, &str, Color); 3],
    pal: &Palette,
    font_sm: &ExoFont<'_>,
    font_md: &ExoFont<'_>,
) {
    cv.fill_rounded_rect(rect, 12, pal.card_bg);
    cv.stroke_rounded_rect(rect, 12, pal.border_subtle);
    cv.draw_text(font_md, rect.x + 14, rect.y + 10, title, pal.text_primary);

    let pad = 16;
    let gap = 20;
    let node_w = ((rect.w - pad * 2 - gap * 2) / 3).max(90);
    let node_h = (rect.h - 54).clamp(54, 86);
    let ny = rect.y + 38;

    let n0 = Rect::new(rect.x + pad, ny, node_w, node_h);
    let n1 = Rect::new(n0.right() + gap, ny, node_w, node_h);
    let n2 = Rect::new(n1.right() + gap, ny, node_w, node_h);
    let rects = [n0, n1, n2];

    let cy = ny as f32 + node_h as f32 * 0.5;
    cv.draw_line_aa(n0.right() as f32, cy, n1.x as f32, cy, 2.2, nodes[0].2);
    cv.draw_line_aa(n1.right() as f32, cy, n2.x as f32, cy, 2.2, nodes[1].2);

    for i in 0..3 {
        let nr = rects[i];
        let (heading, sub, col) = nodes[i];
        cv.fill_rounded_rect(nr, 10, pal.elevated_bg);
        cv.stroke_rounded_rect(nr, 10, col.with_alpha(0x9A));
        cv.fill_circle_aa(nr.x as f32 + 14.0, nr.y as f32 + 18.0, 4.5, col);
        cv.draw_text_ellipsis(font_sm, nr.x + 24, nr.y + 11, heading, nr.w - 30, pal.text_primary);
        cv.draw_text_ellipsis(font_sm, nr.x + 12, nr.y + 34, sub, nr.w - 20, pal.text_secondary);
    }
}
