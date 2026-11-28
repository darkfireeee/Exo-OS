//! Anti-aliased text rendering, straight-RGBA icon blitting, and harmonious wallpapers.

use crate::canvas::Canvas;
use crate::color::{blend_over, blend_over_cov, Color};
use crate::font::ExoFont;
use crate::widgets::Rect;

impl<'a> Canvas<'a> {
    /// Render anti-aliased UTF-8 text with an [`ExoFont`], returning final pen X.
    pub fn draw_text(&mut self, font: &ExoFont<'_>, x: i32, y: i32, text: &str, color: Color) -> i32 {
        if color.a() == 0 {
            return x;
        }
        let mut pen_x = x;
        for ch in text.chars() {
            if let Some(g) = font.glyph(ch) {
                let gy0 = y + g.bearing_y;
                if g.width > 0 && g.height > 0 {
                    let gx0 = pen_x;
                    if gx0 + g.width > self.clip.x
                        && gx0 < self.clip.right()
                        && gy0 + g.height > self.clip.y
                        && gy0 < self.clip.bottom()
                    {
                        for row_i in 0..g.height {
                            let py = gy0 + row_i;
                            if py < self.clip.y || py >= self.clip.bottom() {
                                continue;
                            }
                            let dst_row = (py as usize) * self.stride;
                            let src_row = (row_i as usize) * (g.width as usize);
                            for col_i in 0..g.width {
                                let px = gx0 + col_i;
                                if px < self.clip.x || px >= self.clip.right() {
                                    continue;
                                }
                                let cov = g.alpha[src_row + (col_i as usize)];
                                if cov != 0 {
                                    let idx = dst_row + (px as usize);
                                    if idx < self.pixels.len() {
                                        self.pixels[idx] = blend_over_cov(self.pixels[idx], color, cov);
                                    }
                                }
                            }
                        }
                    }
                }
                pen_x += g.advance;
            }
        }
        pen_x
    }

    /// Draw UTF-8 text truncated with `...` if wider than `max_w`.
    pub fn draw_text_ellipsis(
        &mut self,
        font: &ExoFont<'_>,
        x: i32,
        y: i32,
        text: &str,
        max_w: i32,
        color: Color,
    ) -> i32 {
        if font.measure_width(text) <= max_w {
            return self.draw_text(font, x, y, text, color);
        }
        let ell_w = font.measure_width("...");
        let budget = (max_w - ell_w).max(0);
        let mut pen_x = x;
        let mut used = 0i32;
        for ch in text.chars() {
            let adv = font.glyph(ch).map(|g| g.advance).unwrap_or(0);
            if used + adv > budget {
                break;
            }
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            pen_x = self.draw_text(font, pen_x, y, s, color);
            used += adv;
        }
        self.draw_text(font, pen_x, y, "...", color)
    }

    /// Blit a straight-RGBA (`[r, g, b, a]`) icon onto the canvas at `(dx, dy)` scaled to `dst_w × dst_h`.
    pub fn blit_rgba(
        &mut self,
        dx: i32,
        dy: i32,
        dst_w: i32,
        dst_h: i32,
        src: &[u8],
        src_w: u32,
        src_h: u32,
        opacity: u8,
    ) {
        if dst_w <= 0 || dst_h <= 0 || src_w == 0 || src_h == 0 || opacity == 0 {
            return;
        }
        if src.len() < (src_w as usize) * (src_h as usize) * 4 {
            return;
        }
        let dst_rect = Rect::new(dx, dy, dst_w, dst_h).intersect(self.clip);
        if dst_rect.is_empty() {
            return;
        }
        let step_x = ((src_w as u64) << 16) / (dst_w as u64);
        let step_y = ((src_h as u64) << 16) / (dst_h as u64);

        for py in dst_rect.y..dst_rect.bottom() {
            let sy = ((((py - dy) as u64) * step_y) >> 16).min((src_h - 1) as u64) as usize;
            let dst_row = (py as usize) * self.stride;
            let src_row = sy * (src_w as usize);
            for px in dst_rect.x..dst_rect.right() {
                let sx = ((((px - dx) as u64) * step_x) >> 16).min((src_w - 1) as u64) as usize;
                let off = (src_row + sx) * 4;
                let a = src[off + 3];
                if a == 0 {
                    continue;
                }
                let eff_a = if opacity == 255 {
                    a
                } else {
                    ((a as u32 * opacity as u32 + 127) / 255) as u8
                };
                let c = Color::rgba(src[off], src[off + 1], src[off + 2], eff_a);
                let idx = dst_row + (px as usize);
                if idx < self.pixels.len() {
                    self.pixels[idx] = blend_over(self.pixels[idx], c);
                }
            }
        }
    }

    /// Bilinearly scale an RGB565 wallpaper image to fill `(0, 0, width, height)`.
    pub fn blit_rgb565_wallpaper(&mut self, src: &[u8], src_w: u32, src_h: u32) {
        if src_w < 2 || src_h < 2 || self.width <= 0 || self.height <= 0 {
            return;
        }
        if src.len() < (src_w as usize) * (src_h as usize) * 2 {
            return;
        }
        let r = self.clip;
        let max_sx = ((src_w - 1) as u64) << 8;
        let max_sy = ((src_h - 1) as u64) << 8;
        let denom_x = (self.width - 1).max(1) as u64;
        let denom_y = (self.height - 1).max(1) as u64;

        #[inline(always)]
        fn unpack565(src: &[u8], idx: usize) -> (u32, u32, u32) {
            let v = (src[idx * 2] as u32) | ((src[idx * 2 + 1] as u32) << 8);
            let r5 = (v >> 11) & 0x1F;
            let g6 = (v >> 5) & 0x3F;
            let b5 = v & 0x1F;
            ((r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2))
        }

        let sw = src_w as usize;
        for y in r.y..r.bottom() {
            let fy = ((y as u64) * max_sy) / denom_y;
            let y0 = (fy >> 8) as usize;
            let y1 = (y0 + 1).min((src_h - 1) as usize);
            let wy = (fy & 0xFF) as u32;
            let iwy = 256 - wy;
            let dst_row = (y as usize) * self.stride;
            let r0 = y0 * sw;
            let r1 = y1 * sw;

            for x in r.x..r.right() {
                let fx = ((x as u64) * max_sx) / denom_x;
                let x0 = (fx >> 8) as usize;
                let x1 = (x0 + 1).min((src_w - 1) as usize);
                let wx = (fx & 0xFF) as u32;
                let iwx = 256 - wx;

                let (r00, g00, b00) = unpack565(src, r0 + x0);
                let (r10, g10, b10) = unpack565(src, r0 + x1);
                let (r01, g01, b01) = unpack565(src, r1 + x0);
                let (r11, g11, b11) = unpack565(src, r1 + x1);

                let rt = (r00 * iwx + r10 * wx) >> 8;
                let gt = (g00 * iwx + g10 * wx) >> 8;
                let bt = (b00 * iwx + b10 * wx) >> 8;
                let rb = (r01 * iwx + r11 * wx) >> 8;
                let gb = (g01 * iwx + g11 * wx) >> 8;
                let bb = (b01 * iwx + b11 * wx) >> 8;

                let cr = (rt * iwy + rb * wy) >> 8;
                let cg = (gt * iwy + gb * wy) >> 8;
                let cb = (bt * iwy + bb * wy) >> 8;

                let idx = dst_row + (x as usize);
                if idx < self.pixels.len() {
                    self.pixels[idx] = 0xFF00_0000 | (cr << 16) | (cg << 8) | cb;
                }
            }
        }
    }

    /// Smooth multi-stop harmonious mesh gradient wallpapers (no blue-glass clichés).
    pub fn render_harmonious_wallpaper(&mut self, variant: u8) {
        let w = self.width.max(1);
        let h = self.height.max(1);
        let (tl, tr, bl, br) = match variant {
            // 1: Horizon Clair (soft ivory, mint-mist, warm peach-sand — ideal for Light mode)
            1 => (
                Color::rgb(0xE8, 0xEC, 0xF5),
                Color::rgb(0xD8, 0xEE, 0xEC),
                Color::rgb(0xF6, 0xE6, 0xDC),
                Color::rgb(0xE2, 0xDE, 0xF5),
            ),
            // 2: Aurore Sereine (warm twilight coral, plum, deep teal)
            2 => (
                Color::rgb(0x1B, 0x1D, 0x32),
                Color::rgb(0x13, 0x3B, 0x45),
                Color::rgb(0x3B, 0x20, 0x35),
                Color::rgb(0x4F, 0x31, 0x28),
            ),
            // 3: Ardoise Velours (sober neutral graphite & warm emerald hint)
            _ => (
                Color::rgb(0x14, 0x16, 0x1B),
                Color::rgb(0x1B, 0x23, 0x29),
                Color::rgb(0x1E, 0x1B, 0x24),
                Color::rgb(0x13, 0x22, 0x20),
            ),
        };
        let r = self.clip;
        let denom_x = (w - 1).max(1) as u32;
        let denom_y = (h - 1).max(1) as u32;
        for y in r.y..r.bottom() {
            let ty = (((y as u32) * 255) / denom_y).min(255) as u8;
            let left = tl.lerp(bl, ty);
            let right = tr.lerp(br, ty);
            let row = (y as usize) * self.stride;
            for x in r.x..r.right() {
                let tx = (((x as u32) * 255) / denom_x).min(255) as u8;
                let c = left.lerp(right, tx);
                let idx = row + (x as usize);
                if idx < self.pixels.len() {
                    self.pixels[idx] = 0xFF00_0000 | (c.0 & 0x00FF_FFFF);
                }
            }
        }
    }
}
