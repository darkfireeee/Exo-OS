use crate::fb::{Color, Framebuffer};

pub const GLYPH_W: usize = 8;
pub const GLYPH_H: usize = 16;

pub fn blit_mono_glyph(
    fb: &mut Framebuffer<'_>,
    x: usize,
    y: usize,
    glyph: &[u8; GLYPH_H],
    fg: Color,
    bg: Color,
) {
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..GLYPH_W {
            let mask = 0x80 >> col;
            let color = if bits & mask != 0 { fg } else { bg };
            let _ = fb.put_pixel(x + col, y + row, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::{FramebufferInfo, PixelFormat};

    #[test]
    fn blits_glyph_pixels() {
        let mut pixels = [0u32; 8 * 16];
        let info = FramebufferInfo {
            width: 8,
            height: 16,
            stride_pixels: 8,
            format: PixelFormat::RgbX8888,
        };
        let mut fb = Framebuffer::new(info, &mut pixels).unwrap();
        let mut glyph = [0u8; GLYPH_H];
        glyph[0] = 0x80;
        blit_mono_glyph(&mut fb, 0, 0, &glyph, Color::WHITE, Color::BLACK);
        assert_eq!(pixels[0], Color::WHITE.to_u32(PixelFormat::RgbX8888));
        assert_eq!(pixels[1], Color::BLACK.to_u32(PixelFormat::RgbX8888));
    }
}
