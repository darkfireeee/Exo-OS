//! Vector 2D primitives: rectangles, gradients, SDF rounded rectangles, AA circles/lines/arcs/polygons.

use alloc::vec::Vec;

use crate::canvas::{atan2_f32, isqrt_u64, sqrt_f32, Canvas};
use crate::color::{blend_over, blend_over_cov, blend_over_pre, Color};
use crate::widgets::Rect;

impl<'a> Canvas<'a> {
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let r = rect.intersect(self.clip);
        let a = color.a() as u32;
        if r.is_empty() || a == 0 {
            return;
        }
        let px_len = self.pixels.len();
        if a == 255 {
            let px = 0xFF00_0000 | (color.0 & 0x00FF_FFFF);
            for y in r.y..r.bottom() {
                let row = (y as usize) * self.stride;
                let start = row + (r.x as usize);
                let end = (row + (r.right() as usize)).min(px_len);
                if start < end {
                    self.pixels[start..end].fill(px);
                }
            }
            return;
        }
        let s_rb = color.0 & 0x00FF_00FF;
        let s_g = (color.0 >> 8) & 0xFF;
        let inv = 255 - a;
        for y in r.y..r.bottom() {
            let row = (y as usize) * self.stride;
            let start = row + (r.x as usize);
            let end = (row + (r.right() as usize)).min(px_len);
            if start < end {
                for dst in &mut self.pixels[start..end] {
                    *dst = blend_over_pre(*dst, s_rb, s_g, a, inv);
                }
            }
        }
    }

    pub fn fill_gradient_v(&mut self, rect: Rect, top: Color, bottom: Color) {
        let r = rect.intersect(self.clip);
        if r.is_empty() || rect.h <= 0 {
            return;
        }
        let h_denom = (rect.h - 1).max(1) as u32;
        for y in r.y..r.bottom() {
            let t = (((y - rect.y).max(0) as u32 * 255) / h_denom).min(255) as u8;
            let c = top.lerp(bottom, t);
            let row = (y as usize) * self.stride;
            if c.a() == 255 {
                let px = 0xFF00_0000 | (c.0 & 0x00FF_FFFF);
                let start = row + (r.x as usize);
                let end = (row + (r.right() as usize)).min(self.pixels.len());
                if start < end {
                    self.pixels[start..end].fill(px);
                }
            } else {
                for x in r.x..r.right() {
                    let idx = row + (x as usize);
                    if idx < self.pixels.len() {
                        self.pixels[idx] = blend_over(self.pixels[idx], c);
                    }
                }
            }
        }
    }

    pub fn fill_gradient_h(&mut self, rect: Rect, left: Color, right: Color) {
        let r = rect.intersect(self.clip);
        if r.is_empty() || rect.w <= 0 {
            return;
        }
        let w_denom = (rect.w - 1).max(1) as u32;
        for x in r.x..r.right() {
            let t = (((x - rect.x).max(0) as u32 * 255) / w_denom).min(255) as u8;
            let c = left.lerp(right, t);
            for y in r.y..r.bottom() {
                let idx = (y as usize) * self.stride + (x as usize);
                if idx < self.pixels.len() {
                    self.pixels[idx] = blend_over(self.pixels[idx], c);
                }
            }
        }
    }

    /// SDF anti-aliased rounded rectangle fill.
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: i32, color: Color) {
        if rect.is_empty() || color.a() == 0 {
            return;
        }
        let rad = radius.clamp(0, rect.w.min(rect.h) / 2);
        if rad <= 0 {
            self.fill_rect(rect, color);
            return;
        }
        let clipped = rect.intersect(self.clip);
        if clipped.is_empty() {
            return;
        }

        let mid_y0 = (rect.y + rad).max(clipped.y);
        let mid_y1 = (rect.bottom() - rad).min(clipped.bottom());
        if mid_y0 < mid_y1 {
            self.fill_rect(Rect::new(clipped.x, mid_y0, clipped.w, mid_y1 - mid_y0), color);
        }

        let rad_x2 = (rad * 2) as i64;
        let outer_sq = ((rad_x2 + 1) * (rad_x2 + 1)) as u64;
        let inner_lim = (rad_x2 - 1).max(0);
        let inner_sq = (inner_lim * inner_lim) as u64;
        let target_q4 = (rad_x2 as u64) << 4;
        let a = color.a() as u32;
        let opaque_px = 0xFF00_0000 | (color.0 & 0x00FF_FFFF);
        let s_rb = color.0 & 0x00FF_00FF;
        let s_g = (color.0 >> 8) & 0xFF;
        let inv = 255 - a;
        let px_len = self.pixels.len();

        let bands = [
            (clipped.y, (rect.y + rad).min(clipped.bottom()), true),
            ((rect.bottom() - rad).max(clipped.y), clipped.bottom(), false),
        ];

        for (by0, by1, is_top) in bands {
            if by0 >= by1 {
                continue;
            }
            let cx_l = rect.x + rad;
            let cx_r = rect.right() - rad;
            let cy = if is_top { rect.y + rad } else { rect.bottom() - rad };

            let span_x0 = cx_l.max(clipped.x);
            let span_x1 = cx_r.min(clipped.right());
            if span_x0 < span_x1 {
                self.fill_rect(Rect::new(span_x0, by0, span_x1 - span_x0, by1 - by0), color);
            }

            for y in by0..by1 {
                let dy2 = (2 * y + 1 - 2 * cy) as i64;
                let dy2_sq = (dy2 * dy2) as u64;
                let row = (y as usize) * self.stride;

                let ranges = [
                    (clipped.x.max(rect.x), clipped.right().min(cx_l), cx_l),
                    (clipped.x.max(cx_r), clipped.right().min(rect.right()), cx_r),
                ];
                for (x0, x1, cx) in ranges {
                    for x in x0..x1 {
                        let dx2 = (2 * x + 1 - 2 * cx) as i64;
                        let d2 = (dx2 * dx2) as u64 + dy2_sq;
                        let idx = row + (x as usize);
                        if idx >= px_len {
                            continue;
                        }
                        if d2 <= inner_sq {
                            self.pixels[idx] = if a == 255 {
                                opaque_px
                            } else {
                                blend_over_pre(self.pixels[idx], s_rb, s_g, a, inv)
                            };
                        } else if d2 < outer_sq {
                            let dist_q4 = isqrt_u64(d2 << 8);
                            let cov = if dist_q4 <= target_q4.saturating_sub(16) {
                                255
                            } else if dist_q4 >= target_q4 + 16 {
                                0
                            } else {
                                (((target_q4 + 16 - dist_q4) * 255) >> 5) as u8
                            };
                            self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                        }
                    }
                }
            }
        }
    }

    pub fn stroke_rounded_rect(&mut self, rect: Rect, radius: i32, color: Color) {
        if rect.w < 2 || rect.h < 2 || color.a() == 0 {
            return;
        }
        let rad = radius.clamp(0, rect.w.min(rect.h) / 2);
        self.fill_rect(Rect::new(rect.x + rad, rect.y, rect.w - 2 * rad, 1), color);
        self.fill_rect(Rect::new(rect.x + rad, rect.bottom() - 1, rect.w - 2 * rad, 1), color);
        self.fill_rect(Rect::new(rect.x, rect.y + rad, 1, rect.h - 2 * rad), color);
        self.fill_rect(Rect::new(rect.right() - 1, rect.y + rad, 1, rect.h - 2 * rad), color);
        if rad <= 0 {
            return;
        }
        let r_f = rad as f32 - 0.5;
        let min_r = (r_f - 1.1).max(0.0);
        let max_r = r_f + 1.1;
        let min_d2 = min_r * min_r;
        let max_d2 = max_r * max_r;
        let corners = [
            (rect.x, rect.y, rect.x + rad, rect.y + rad),
            (rect.right() - rad, rect.y, rect.right() - rad, rect.y + rad),
            (rect.x, rect.bottom() - rad, rect.x + rad, rect.bottom() - rad),
            (rect.right() - rad, rect.bottom() - rad, rect.right() - rad, rect.bottom() - rad),
        ];
        for (bx, by, cx, cy) in corners {
            let cr = Rect::new(bx, by, rad, rad).intersect(self.clip);
            if cr.is_empty() {
                continue;
            }
            for y in cr.y..cr.bottom() {
                let dy = (y as f32 + 0.5) - cy as f32;
                let dy2 = dy * dy;
                let row = (y as usize) * self.stride;
                for x in cr.x..cr.right() {
                    let dx = (x as f32 + 0.5) - cx as f32;
                    let d2 = dx * dx + dy2;
                    if d2 < min_d2 || d2 > max_d2 {
                        continue;
                    }
                    let diff = (sqrt_f32(d2) - r_f).abs();
                    if diff < 1.1 {
                        let cov = ((1.0 - diff * (1.0 / 1.1)) * 255.0).clamp(0.0, 255.0) as u8;
                        let idx = row + (x as usize);
                        if idx < self.pixels.len() {
                            self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                        }
                    }
                }
            }
        }
    }

    pub fn fill_circle_aa(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        if radius <= 0.0 || color.a() == 0 {
            return;
        }
        let r = Rect::new(
            (cx - radius - 1.5) as i32,
            (cy - radius - 1.5) as i32,
            ((radius + 2.0) * 2.0) as i32,
            ((radius + 2.0) * 2.0) as i32,
        )
        .intersect(self.clip);
        for y in r.y..r.bottom() {
            let dy = (y as f32 + 0.5) - cy;
            for x in r.x..r.right() {
                let dx = (x as f32 + 0.5) - cx;
                let d = sqrt_f32(dx * dx + dy * dy);
                if d <= radius - 0.5 {
                    self.blend_px(x, y, color);
                } else if d < radius + 0.5 {
                    let cov = ((radius + 0.5 - d) * 255.0).clamp(0.0, 255.0) as u8;
                    self.blend_px_cov(x, y, color, cov);
                }
            }
        }
    }

    /// Major-axis strip anti-aliased line rasterizer (30x-40x faster than full AABB scan for charts/radar).
    pub fn draw_line_aa(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, color: Color) {
        if color.a() == 0 {
            return;
        }
        let half_t = (thickness * 0.5).max(0.5);
        let vx = x1 - x0;
        let vy = y1 - y0;
        let len_sq = vx * vx + vy * vy;
        if len_sq < 0.0001 {
            self.fill_circle_aa(x0, y0, half_t, color);
            return;
        }
        let avx = if vx < 0.0 { -vx } else { vx };
        let avy = if vy < 0.0 { -vy } else { vy };
        // Exact fast-path for horizontal & vertical segments
        if avy < 0.02 {
            let lx = (if x0 < x1 { x0 } else { x1 }) as i32;
            let rx = (if x0 > x1 { x0 } else { x1 } + 0.5) as i32;
            let ty = (y0 - half_t + 0.5) as i32;
            let th = (thickness + 0.5).max(1.0) as i32;
            self.fill_rect(Rect::new(lx, ty, (rx - lx).max(1), th), color);
            return;
        }
        if avx < 0.02 {
            let ty = (if y0 < y1 { y0 } else { y1 }) as i32;
            let by = (if y0 > y1 { y0 } else { y1 } + 0.5) as i32;
            let lx = (x0 - half_t + 0.5) as i32;
            let tw = (thickness + 0.5).max(1.0) as i32;
            self.fill_rect(Rect::new(lx, ty, tw, (by - ty).max(1)), color);
            return;
        }

        let inv_len_sq = 1.0 / len_sq;
        let pad = half_t + 1.5;
        let max_d = half_t + 0.5;
        let max_d_sq = max_d * max_d;
        let min_d = (half_t - 0.5).max(0.0);
        let min_d_sq = min_d * min_d;

        if avx >= avy {
            // Walk X, test only tight Y strip around the line
            let (ax, ay, bx, by) = if x0 <= x1 { (x0, y0, x1, y1) } else { (x1, y1, x0, y0) };
            let slope = (by - ay) / (bx - ax);
            let band_y = (pad * sqrt_f32(1.0 + slope * slope)) + 1.0;
            let xs = ((ax - pad) as i32).max(self.clip.x);
            let xe = ((bx + pad + 1.0) as i32).min(self.clip.right());
            for x in xs..xe {
                let px = x as f32 + 0.5;
                let clamped_px = px.clamp(ax, bx);
                let cy = ay + (clamped_px - ax) * slope;
                let ys = ((cy - band_y) as i32).max(self.clip.y);
                let ye = ((cy + band_y + 1.0) as i32).min(self.clip.bottom());
                for y in ys..ye {
                    let py = y as f32 + 0.5;
                    let t = (((px - x0) * vx + (py - y0) * vy) * inv_len_sq).clamp(0.0, 1.0);
                    let dx = px - (x0 + t * vx);
                    let dy = py - (y0 + t * vy);
                    let d2 = dx * dx + dy * dy;
                    if d2 >= max_d_sq {
                        continue;
                    }
                    let idx = (y as usize) * self.stride + (x as usize);
                    if idx >= self.pixels.len() {
                        continue;
                    }
                    if d2 <= min_d_sq {
                        self.pixels[idx] = blend_over(self.pixels[idx], color);
                    } else {
                        let cov = ((max_d - sqrt_f32(d2)) * 255.0).clamp(0.0, 255.0) as u8;
                        self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                    }
                }
            }
        } else {
            // Walk Y, test only tight X strip around the line
            let (ax, ay, bx, by) = if y0 <= y1 { (x0, y0, x1, y1) } else { (x1, y1, x0, y0) };
            let inv_slope = (bx - ax) / (by - ay);
            let band_x = (pad * sqrt_f32(1.0 + inv_slope * inv_slope)) + 1.0;
            let ys = ((ay - pad) as i32).max(self.clip.y);
            let ye = ((by + pad + 1.0) as i32).min(self.clip.bottom());
            for y in ys..ye {
                let py = y as f32 + 0.5;
                let clamped_py = py.clamp(ay, by);
                let cx = ax + (clamped_py - ay) * inv_slope;
                let xs = ((cx - band_x) as i32).max(self.clip.x);
                let xe = ((cx + band_x + 1.0) as i32).min(self.clip.right());
                let row = (y as usize) * self.stride;
                for x in xs..xe {
                    let px = x as f32 + 0.5;
                    let t = (((px - x0) * vx + (py - y0) * vy) * inv_len_sq).clamp(0.0, 1.0);
                    let dx = px - (x0 + t * vx);
                    let dy = py - (y0 + t * vy);
                    let d2 = dx * dx + dy * dy;
                    if d2 >= max_d_sq {
                        continue;
                    }
                    let idx = row + (x as usize);
                    if idx >= self.pixels.len() {
                        continue;
                    }
                    if d2 <= min_d_sq {
                        self.pixels[idx] = blend_over(self.pixels[idx], color);
                    } else {
                        let cov = ((max_d - sqrt_f32(d2)) * 255.0).clamp(0.0, 255.0) as u8;
                        self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                    }
                }
            }
        }
    }

    pub fn stroke_arc_aa(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        thickness: f32,
        start_deg: f32,
        sweep_deg: f32,
        color: Color,
    ) {
        if sweep_deg <= 0.0 || radius <= 1.0 || color.a() == 0 {
            return;
        }
        let half_t = (thickness * 0.5).max(0.5);
        let r_out = radius + half_t + 0.7;
        let r_in = (radius - half_t - 0.7).max(0.0);
        let r_in_sq = r_in * r_in;
        let r_out_sq = r_out * r_out;
        let r = Rect::new(
            (cx - r_out - 0.5) as i32,
            (cy - r_out - 0.5) as i32,
            (r_out * 2.0 + 2.0) as i32,
            (r_out * 2.0 + 2.0) as i32,
        )
        .intersect(self.clip);
        if r.is_empty() {
            return;
        }
        let full = sweep_deg >= 359.9;
        let start_norm = ((start_deg % 360.0) + 360.0) % 360.0;
        let a = color.a() as u32;
        let opaque_px = 0xFF00_0000 | (color.0 & 0x00FF_FFFF);
        let s_rb = color.0 & 0x00FF_00FF;
        let s_g = (color.0 >> 8) & 0xFF;
        let inv = 255 - a;
        let px_len = self.pixels.len();

        for y in r.y..r.bottom() {
            let dy = (y as f32 + 0.5) - cy;
            let dy2 = dy * dy;
            if dy2 > r_out_sq {
                continue;
            }
            let row = (y as usize) * self.stride;
            for x in r.x..r.right() {
                let dx = (x as f32 + 0.5) - cx;
                let d2 = dx * dx + dy2;
                if d2 < r_in_sq || d2 > r_out_sq {
                    continue;
                }
                let radial_dist = (sqrt_f32(d2) - radius).abs();
                if radial_dist >= half_t + 0.6 {
                    continue;
                }
                if !full {
                    let mut deg = atan2_f32(dx, -dy) * (180.0 / core::f32::consts::PI);
                    if deg < 0.0 {
                        deg += 360.0;
                    }
                    let mut rel = deg - start_norm;
                    if rel < 0.0 {
                        rel += 360.0;
                    }
                    if rel > sweep_deg {
                        continue;
                    }
                }
                let idx = row + (x as usize);
                if idx >= px_len {
                    continue;
                }
                if radial_dist <= half_t - 0.5 {
                    self.pixels[idx] = if a == 255 {
                        opaque_px
                    } else {
                        blend_over_pre(self.pixels[idx], s_rb, s_g, a, inv)
                    };
                } else {
                    let cov = ((half_t + 0.6 - radial_dist) * (255.0 / 1.1)).clamp(0.0, 255.0) as u8;
                    self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                }
            }
        }
    }

    pub fn fill_polygon(&mut self, pts: &[(f32, f32)], color: Color) {
        let a = color.a() as u32;
        if pts.len() < 3 || a == 0 {
            return;
        }
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for &(_, py) in pts {
            min_y = min_y.min(py as i32);
            max_y = max_y.max(py as i32 + 1);
        }
        min_y = min_y.max(self.clip.y);
        max_y = max_y.min(self.clip.bottom());
        if min_y >= max_y {
            return;
        }
        let mut nodes: Vec<f32> = Vec::with_capacity(16);
        let opaque_px = 0xFF00_0000 | (color.0 & 0x00FF_FFFF);
        let s_rb = color.0 & 0x00FF_00FF;
        let s_g = (color.0 >> 8) & 0xFF;
        let inv = 255 - a;
        let px_len = self.pixels.len();

        for y in min_y..max_y {
            let fy = y as f32 + 0.5;
            nodes.clear();
            let n = pts.len();
            let mut j = n - 1;
            for i in 0..n {
                let (xi, yi) = pts[i];
                let (xj, yj) = pts[j];
                if (yi < fy && yj >= fy) || (yj < fy && yi >= fy) {
                    nodes.push(xi + (fy - yi) / (yj - yi) * (xj - xi));
                }
                j = i;
            }
            nodes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
            let row = (y as usize) * self.stride;
            let mut k = 0usize;
            while k + 1 < nodes.len() {
                let x0 = (nodes[k] as i32).max(self.clip.x);
                let x1 = ((nodes[k + 1] + 0.5) as i32).min(self.clip.right());
                if x0 < x1 {
                    let start = row + (x0 as usize);
                    let end = (row + (x1 as usize)).min(px_len);
                    if start < end {
                        if a == 255 {
                            self.pixels[start..end].fill(opaque_px);
                        } else {
                            for dst in &mut self.pixels[start..end] {
                                *dst = blend_over_pre(*dst, s_rb, s_g, a, inv);
                            }
                        }
                    }
                }
                k += 2;
            }
        }
    }
}
