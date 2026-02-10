//! CRC and checksum utilities

/// CRC32 checksum calculation
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;

    for &byte in data {
        crc ^= byte as u32;

        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }

    !crc
}

/// CRC32C (Castagnoli) - used by ext4
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    const POLY: u32 = 0x82F6_3B78;

    for &byte in data {
        crc ^= byte as u32;

        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
        }
    }

    !crc
}

/// Simple checksum (sum of bytes)
pub fn simple_checksum(data: &[u8]) -> u32 {
    data.iter().map(|&b| b as u32).sum()
}

/// Fletcher-16 checksum
pub fn fletcher16(data: &[u8]) -> u16 {
    let mut sum1: u16 = 0;
    let mut sum2: u16 = 0;

    for &byte in data {
        sum1 = (sum1 + byte as u16) % 255;
        sum2 = (sum2 + sum1) % 255;
    }

    (sum2 << 8) | sum1
}

/// Adler-32 checksum
pub fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;

    let mut a: u32 = 1;
    let mut b: u32 = 0;

    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }

    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32() {
        let data = b"hello world";
        let crc = crc32(data);
        assert_ne!(crc, 0);
    }

    #[test]
    fn test_adler32() {
        let data = b"Wikipedia";
        let checksum = adler32(data);
        assert_eq!(checksum, 0x11E60398);
    }
}
