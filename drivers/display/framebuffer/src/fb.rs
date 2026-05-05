#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    RgbX8888,
    BgrX8888,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self {
        r: 0xff,
        g: 0xff,
        b: 0xff,
    };
    pub const GREEN: Self = Self {
        r: 0x60,
        g: 0xff,
        b: 0x90,
    };

    pub const fn to_u32(self, format: PixelFormat) -> u32 {
        match format {
            PixelFormat::RgbX8888 => {
                (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
            }
            PixelFormat::BgrX8888 => {
                (self.b as u32) | ((self.g as u32) << 8) | ((self.r as u32) << 16)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferInfo {
    pub width: usize,
    pub height: usize,
    pub stride_pixels: usize,
    pub format: PixelFormat,
}

pub struct Framebuffer<'a> {
    info: FramebufferInfo,
    pixels: &'a mut [u32],
}

impl<'a> Framebuffer<'a> {
    pub fn new(info: FramebufferInfo, pixels: &'a mut [u32]) -> Option<Self> {
        if info.width == 0 || info.height == 0 || info.stride_pixels < info.width {
            return None;
        }
        let needed = info.stride_pixels.checked_mul(info.height)?;
        if pixels.len() < needed {
            return None;
        }
        Some(Self { info, pixels })
    }

    pub const fn info(&self) -> FramebufferInfo {
        self.info
    }

    pub fn clear(&mut self, color: Color) {
        let raw = color.to_u32(self.info.format);
        for y in 0..self.info.height {
            let row = y * self.info.stride_pixels;
            for x in 0..self.info.width {
                self.pixels[row + x] = raw;
            }
        }
    }

    pub fn put_pixel(&mut self, x: usize, y: usize, color: Color) -> bool {
        if x >= self.info.width || y >= self.info.height {
            return false;
        }
        self.pixels[y * self.info.stride_pixels + x] = color.to_u32(self.info.format);
        true
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let x_end = core::cmp::min(self.info.width, x.saturating_add(w));
        let y_end = core::cmp::min(self.info.height, y.saturating_add(h));
        let raw = color.to_u32(self.info.format);
        for yy in y..y_end {
            let row = yy * self.info.stride_pixels;
            for xx in x..x_end {
                self.pixels[row + xx] = raw;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_inside_bounds_only() {
        let mut pixels = [0u32; 16];
        let info = FramebufferInfo {
            width: 4,
            height: 4,
            stride_pixels: 4,
            format: PixelFormat::RgbX8888,
        };
        let mut fb = Framebuffer::new(info, &mut pixels).unwrap();
        assert!(fb.put_pixel(1, 1, Color::WHITE));
        assert!(!fb.put_pixel(4, 1, Color::WHITE));
        assert_eq!(pixels[5], Color::WHITE.to_u32(PixelFormat::RgbX8888));
    }
}
