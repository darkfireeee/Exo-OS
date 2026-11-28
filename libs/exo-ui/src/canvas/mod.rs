//! Clip-aware ARGB8888 2D renderer — modularized into `math`, `primitives`, `effects`, and `image`.

mod effects;
mod image;
mod math;
mod primitives;

pub use math::{atan2_f32, sin_cos_f32, sqrt_f32};
pub(crate) use math::isqrt_u64;

use crate::color::{blend_over, blend_over_cov, Color};
use crate::widgets::Rect;

pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub clip: Rect,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32, stride: usize) -> Self {
        let w = width as i32;
        let h = height as i32;
        Self { pixels, width: w, height: h, stride, clip: Rect::new(0, 0, w, h) }
    }

    pub fn push_clip(&mut self, r: Rect) -> Rect {
        let prev = self.clip;
        self.clip = self.clip.intersect(r);
        prev
    }

    #[inline(always)]
    pub fn blend_px(&mut self, x: i32, y: i32, color: Color) {
        if x < self.clip.x || y < self.clip.y || x >= self.clip.right() || y >= self.clip.bottom() {
            return;
        }
        let idx = (y as usize) * self.stride + (x as usize);
        if idx < self.pixels.len() {
            self.pixels[idx] = blend_over(self.pixels[idx], color);
        }
    }

    #[inline(always)]
    pub fn blend_px_cov(&mut self, x: i32, y: i32, color: Color, cov: u8) {
        if cov == 0 || x < self.clip.x || y < self.clip.y || x >= self.clip.right() || y >= self.clip.bottom() {
            return;
        }
        let idx = (y as usize) * self.stride + (x as usize);
        if idx < self.pixels.len() {
            self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
        }
    }

    pub fn clear(&mut self, color: Color) {
        let px = 0xFF00_0000 | (color.0 & 0x00FF_FFFF);
        let r = self.clip;
        for y in r.y..r.bottom() {
            let row = (y as usize) * self.stride;
            let start = row + (r.x as usize);
            let end = (row + (r.right() as usize)).min(self.pixels.len());
            if start < end { self.pixels[start..end].fill(px); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Theme;
    use alloc::vec;

    #[test]
    fn panel_and_primitives_do_not_panic() {
        let mut buf = vec![0xFF00_0000u32; 128 * 96];
        let mut cv = Canvas::new(&mut buf, 128, 96, 128);
        let pal = Theme::new().palette();
        cv.draw_panel(Rect::new(8, 8, 112, 80), 12, pal.window_bg, pal.border_strong, true, true);
        cv.draw_line_aa(10.0, 10.0, 118.0, 86.0, 2.0, pal.accent);
        cv.stroke_arc_aa(64.0, 48.0, 24.0, 5.0, 0.0, 270.0, pal.success);
        assert_ne!(buf[48 * 128 + 64], 0xFF00_0000);
    }
}
