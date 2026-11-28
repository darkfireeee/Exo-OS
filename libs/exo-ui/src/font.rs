//! Pre-baked 8-bit anti-aliased UTF-8 font atlas decoder (`.exofont`).
//!
//! Binary format (`EXOF` v1, little-endian):
//! - Header (16 B): `magic[4] = b"EXOF"`, `line_height: u16`, `ascent: u16`,
//!   `glyph_count: u16`, `reserved: u16`, `bitmap_bytes: u32`.
//! - Table (`glyph_count * 12 B`): `codepoint: u32`, `advance: u8`, `width: u8`,
//!   `height: u8`, `bearing_y: i8`, `offset: u32` (relative to start of bitmap blob).
//! - Bitmap blob: packed 8-bit coverage alpha (`0..=255`) per glyph.

/// Decoded glyph metrics & slice into the 8-bit alpha coverage atlas.
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    pub advance: i32,
    pub width: i32,
    pub height: i32,
    pub bearing_y: i32,
    pub alpha: &'a [u8],
}

/// Zero-copy view over a `.exofont` pre-baked anti-aliased font binary.
#[derive(Clone, Copy)]
pub struct ExoFont<'a> {
    data: &'a [u8],
    pub line_height: i32,
    pub ascent: i32,
    glyph_count: usize,
    bitmap_base: usize,
}

impl<'a> ExoFont<'a> {
    const HEADER_SIZE: usize = 16;
    const ENTRY_SIZE: usize = 12;

    pub fn from_bytes(data: &'a [u8]) -> Option<Self> {
        if data.len() < Self::HEADER_SIZE || &data[0..4] != b"EXOF" {
            return None;
        }
        let line_height = u16::from_le_bytes([data[4], data[5]]) as i32;
        let ascent = u16::from_le_bytes([data[6], data[7]]) as i32;
        let glyph_count = u16::from_le_bytes([data[8], data[9]]) as usize;
        let bitmap_bytes = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
        let bitmap_base = Self::HEADER_SIZE.checked_add(glyph_count.checked_mul(Self::ENTRY_SIZE)?)?;
        if bitmap_base.checked_add(bitmap_bytes)? > data.len() || line_height <= 0 {
            return None;
        }
        Some(Self {
            data,
            line_height,
            ascent,
            glyph_count,
            bitmap_base,
        })
    }

    #[inline]
    fn entry_at(&self, idx: usize) -> Option<(u32, Glyph<'a>)> {
        if idx >= self.glyph_count {
            return None;
        }
        let off = Self::HEADER_SIZE + idx * Self::ENTRY_SIZE;
        let cp = u32::from_le_bytes([
            self.data[off],
            self.data[off + 1],
            self.data[off + 2],
            self.data[off + 3],
        ]);
        let advance = self.data[off + 4] as i32;
        let width = self.data[off + 5] as i32;
        let height = self.data[off + 6] as i32;
        let bearing_y = self.data[off + 7] as i8 as i32;
        let bmp_off = u32::from_le_bytes([
            self.data[off + 8],
            self.data[off + 9],
            self.data[off + 10],
            self.data[off + 11],
        ]) as usize;
        let byte_len = (width.max(0) as usize).checked_mul(height.max(0) as usize)?;
        let start = self.bitmap_base.checked_add(bmp_off)?;
        let end = start.checked_add(byte_len)?;
        if end > self.data.len() {
            return None;
        }
        Some((
            cp,
            Glyph {
                advance,
                width,
                height,
                bearing_y,
                alpha: &self.data[start..end],
            },
        ))
    }

    /// Binary-search lookup by Unicode scalar value, falling back to `'?'`.
    pub fn glyph(&self, ch: char) -> Option<Glyph<'a>> {
        let target = ch as u32;
        let mut lo = 0usize;
        let mut hi = self.glyph_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (cp, g) = self.entry_at(mid)?;
            if cp == target {
                return Some(g);
            } else if cp < target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        // Fallback to '?'
        if ch != '?' {
            self.glyph('?')
        } else {
            self.entry_at(0).map(|(_, g)| g)
        }
    }

    /// Measure total pixel advance width of a UTF-8 string.
    pub fn measure_width(&self, text: &str) -> i32 {
        let mut w = 0i32;
        for ch in text.chars() {
            if let Some(g) = self.glyph(ch) {
                w += g.advance;
            }
        }
        w
    }

    /// Map an X pixel offset relative to text start to the nearest UTF-8 byte boundary (for mouse caret/selection).
    pub fn hit_test_byte_index(&self, text: &str, rel_x: i32) -> usize {
        if rel_x <= 0 {
            return 0;
        }
        let mut x = 0i32;
        for (byte_idx, ch) in text.char_indices() {
            let adv = self.glyph(ch).map(|g| g.advance).unwrap_or(8);
            if rel_x < x + adv / 2 {
                return byte_idx;
            }
            x += adv;
        }
        text.len()
    }

    /// Return X pixel offset of a given UTF-8 byte index inside `text`.
    pub fn x_of_byte_index(&self, text: &str, byte_idx: usize) -> i32 {
        let clamped = byte_idx.min(text.len());
        let mut x = 0i32;
        for (idx, ch) in text.char_indices() {
            if idx >= clamped {
                break;
            }
            x += self.glyph(ch).map(|g| g.advance).unwrap_or(8);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn parses_synthetic_exofont_and_hit_tests() {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"EXOF");
        blob.extend_from_slice(&16u16.to_le_bytes()); // line_height
        blob.extend_from_slice(&13u16.to_le_bytes()); // ascent
        blob.extend_from_slice(&2u16.to_le_bytes()); // glyph_count
        blob.extend_from_slice(&0u16.to_le_bytes());
        blob.extend_from_slice(&8u32.to_le_bytes()); // bitmap bytes
        // Glyph 'A' (65)
        blob.extend_from_slice(&65u32.to_le_bytes());
        blob.extend_from_slice(&[10, 2, 2, 0]);
        blob.extend_from_slice(&0u32.to_le_bytes());
        // Glyph 'B' (66)
        blob.extend_from_slice(&66u32.to_le_bytes());
        blob.extend_from_slice(&[10, 2, 2, 0]);
        blob.extend_from_slice(&4u32.to_le_bytes());
        // Bitmaps
        blob.extend_from_slice(&[255, 128, 128, 255, 64, 255, 255, 64]);

        let font = ExoFont::from_bytes(&blob).expect("valid font");
        assert_eq!(font.measure_width("AB"), 20);
        assert_eq!(font.hit_test_byte_index("AB", 3), 0);
        assert_eq!(font.hit_test_byte_index("AB", 7), 1);
        assert_eq!(font.hit_test_byte_index("AB", 18), 2);
        assert_eq!(font.x_of_byte_index("AB", 1), 10);
    }
}
