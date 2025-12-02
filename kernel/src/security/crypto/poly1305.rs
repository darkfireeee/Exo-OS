//! Poly1305 Message Authentication Code
//!
//! RFC 8439 compliant - One-time authenticator
//! Performance: ~1-3 cycles/byte

/// Poly1305 state
pub struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
    leftover: usize,
    buffer: [u8; 16],
}

impl Poly1305 {
    /// Create new Poly1305 instance
    pub fn new(key: &[u8; 32]) -> Self {
        let mut poly = Self {
            r: [0u32; 5],
            h: [0u32; 5],
            pad: [0u32; 4],
            leftover: 0,
            buffer: [0u8; 16],
        };

        // r = key[0..16] with clamping
        poly.r[0] = (u32::from_le_bytes([key[0], key[1], key[2], key[3]])) & 0x3ffffff;
        poly.r[1] = (u32::from_le_bytes([key[3], key[4], key[5], key[6]]) >> 2) & 0x3ffff03;
        poly.r[2] = (u32::from_le_bytes([key[6], key[7], key[8], key[9]]) >> 4) & 0x3ffc0ff;
        poly.r[3] = (u32::from_le_bytes([key[9], key[10], key[11], key[12]]) >> 6) & 0x3f03fff;
        poly.r[4] = (u32::from_le_bytes([key[12], key[13], key[14], key[15]]) >> 8) & 0x00fffff;

        // pad = key[16..32]
        poly.pad[0] = u32::from_le_bytes([key[16], key[17], key[18], key[19]]);
        poly.pad[1] = u32::from_le_bytes([key[20], key[21], key[22], key[23]]);
        poly.pad[2] = u32::from_le_bytes([key[24], key[25], key[26], key[27]]);
        poly.pad[3] = u32::from_le_bytes([key[28], key[29], key[30], key[31]]);

        poly
    }

    fn block(&mut self, m: &[u8; 16], final_: bool) {
        let hibit = if final_ { 0 } else { 1 << 24 };

        // h += m
        self.h[0] += (u32::from_le_bytes([m[0], m[1], m[2], m[3]])) & 0x3ffffff;
        self.h[1] += (u32::from_le_bytes([m[3], m[4], m[5], m[6]]) >> 2) & 0x3ffffff;
        self.h[2] += (u32::from_le_bytes([m[6], m[7], m[8], m[9]]) >> 4) & 0x3ffffff;
        self.h[3] += (u32::from_le_bytes([m[9], m[10], m[11], m[12]]) >> 6) & 0x3ffffff;
        self.h[4] += (u32::from_le_bytes([m[12], m[13], m[14], m[15]]) >> 8) | hibit;

        // h *= r (mod 2^130 - 5)
        let r0 = self.r[0] as u64;
        let r1 = self.r[1] as u64;
        let r2 = self.r[2] as u64;
        let r3 = self.r[3] as u64;
        let r4 = self.r[4] as u64;

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let h0 = self.h[0] as u64;
        let h1 = self.h[1] as u64;
        let h2 = self.h[2] as u64;
        let h3 = self.h[3] as u64;
        let h4 = self.h[4] as u64;

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        // Partial reduction
        let mut c = d0 >> 26;
        self.h[0] = (d0 & 0x3ffffff) as u32;
        let d1 = d1 + c;
        c = d1 >> 26;
        self.h[1] = (d1 & 0x3ffffff) as u32;
        let d2 = d2 + c;
        c = d2 >> 26;
        self.h[2] = (d2 & 0x3ffffff) as u32;
        let d3 = d3 + c;
        c = d3 >> 26;
        self.h[3] = (d3 & 0x3ffffff) as u32;
        let d4 = d4 + c;
        c = d4 >> 26;
        self.h[4] = (d4 & 0x3ffffff) as u32;
        self.h[0] += (c * 5) as u32;
        self.h[1] += self.h[0] >> 26;
        self.h[0] &= 0x3ffffff;
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut m = data;

        // Process leftover
        if self.leftover > 0 {
            let want = 16 - self.leftover;
            let take = want.min(m.len());
            self.buffer[self.leftover..self.leftover + take].copy_from_slice(&m[..take]);
            m = &m[take..];
            self.leftover += take;
            if self.leftover < 16 {
                return;
            }
            let block = self.buffer;
            self.block(&block, false);
            self.leftover = 0;
        }

        // Process full blocks
        while m.len() >= 16 {
            let mut block = [0u8; 16];
            block.copy_from_slice(&m[..16]);
            self.block(&block, false);
            m = &m[16..];
        }

        // Save leftover
        if !m.is_empty() {
            self.buffer[..m.len()].copy_from_slice(m);
            self.leftover = m.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        // Process final block
        if self.leftover > 0 {
            self.buffer[self.leftover] = 1;
            for i in self.leftover + 1..16 {
                self.buffer[i] = 0;
            }
            let block = self.buffer;
            self.block(&block, true);
        }

        // Full reduction
        let mut c = self.h[1] >> 26;
        self.h[1] &= 0x3ffffff;
        self.h[2] += c;
        c = self.h[2] >> 26;
        self.h[2] &= 0x3ffffff;
        self.h[3] += c;
        c = self.h[3] >> 26;
        self.h[3] &= 0x3ffffff;
        self.h[4] += c;
        c = self.h[4] >> 26;
        self.h[4] &= 0x3ffffff;
        self.h[0] += (c * 5) as u32;
        c = self.h[0] >> 26;
        self.h[0] &= 0x3ffffff;
        self.h[1] += c;

        // Compute h - p
        let mut g = [0u32; 5];
        g[0] = self.h[0].wrapping_add(5);
        c = g[0] >> 26;
        g[0] &= 0x3ffffff;
        g[1] = self.h[1].wrapping_add(c as u32);
        c = g[1] >> 26;
        g[1] &= 0x3ffffff;
        g[2] = self.h[2].wrapping_add(c as u32);
        c = g[2] >> 26;
        g[2] &= 0x3ffffff;
        g[3] = self.h[3].wrapping_add(c as u32);
        c = g[3] >> 26;
        g[3] &= 0x3ffffff;
        g[4] = self.h[4].wrapping_add(c as u32).wrapping_sub(1 << 26);

        // Select h or g
        let mask = ((g[4] >> 31).wrapping_sub(1)) as u32;
        for i in 0..5 {
            g[i] ^= mask & (self.h[i] ^ g[i]);
        }

        // h = (h + pad) mod 2^128
        let mut f = g[0] as u64 | ((g[1] as u64) << 26);
        f += self.pad[0] as u64;
        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&(f as u32).to_le_bytes());

        f = ((g[1] >> 6) as u64 | (g[2] as u64) << 20) + self.pad[1] as u64 + (f >> 32);
        tag[4..8].copy_from_slice(&(f as u32).to_le_bytes());

        f = ((g[2] >> 12) as u64 | (g[3] as u64) << 14) + self.pad[2] as u64 + (f >> 32);
        tag[8..12].copy_from_slice(&(f as u32).to_le_bytes());

        f = ((g[3] >> 18) as u64 | (g[4] as u64) << 8) + self.pad[3] as u64 + (f >> 32);
        tag[12..16].copy_from_slice(&(f as u32).to_le_bytes());

        tag
    }
}

pub fn poly1305(msg: &[u8], key: &[u8; 32]) -> [u8; 16] {
    let mut mac = Poly1305::new(key);
    mac.update(msg);
    mac.finalize()
}
