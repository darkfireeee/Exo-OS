//! CRC-32 (IEEE 802.3, réfléchi, poly 0xEDB88320) — utilisé pour valider le
//! header GPT et la table de partitions. C'est un **checksum**, pas de la crypto.

/// CRC-32 standard (init 0xFFFFFFFF, XOR final 0xFFFFFFFF). Implémentation
/// bit-à-bit (pas de table → aucune donnée statique, déterministe).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg(); // 0xFFFFFFFF si bit bas = 1
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        // Vecteur de référence : CRC-32 de "123456789" == 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc32_detects_single_bit_flip() {
        let a = crc32(b"EFI PART header bytes");
        let mut data = *b"EFI PART header bytes";
        data[0] ^= 1;
        assert_ne!(a, crc32(&data));
    }
}
