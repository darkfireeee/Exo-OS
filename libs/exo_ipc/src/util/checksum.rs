// libs/exo_ipc/src/util/checksum.rs
//! Implémentation de checksums pour validation d'intégrité

/// Calcule un checksum CRC32C simple (sera optimisé avec SIMD)
/// 
/// Note: Cette implémentation est un placeholder.
/// Une version optimisée utiliserait SSE4.2 (CRC32C instruction)
/// ou une table de lookup pour de meilleures performances.
pub fn crc32c(data: &[u8]) -> u32 {
    crc32c_simple(data)
}

/// Implémentation simple de CRC32C (Castagnoli polynomial)
fn crc32c_simple(data: &[u8]) -> u32 {
    const POLY: u32 = 0x82F63B78; // Castagnoli polynomial
    
    let mut crc: u32 = 0xFFFFFFFF;
    
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = ((crc & 1) as u32).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
        }
    }
    
    !crc
}

/// Version optimisée avec table de lookup
#[allow(dead_code)]
fn crc32c_table(data: &[u8]) -> u32 {
    // Table de lookup générée pour le polynomial CRC32C
    static CRC32C_TABLE: [u32; 256] = generate_crc32c_table();
    
    let mut crc: u32 = 0xFFFFFFFF;
    
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32C_TABLE[index];
    }
    
    !crc
}

/// Génère la table de lookup pour CRC32C
const fn generate_crc32c_table() -> [u32; 256] {
    const POLY: u32 = 0x82F63B78;
    let mut table = [0u32; 256];
    let mut i = 0;
    
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        
        while j < 8 {
            let mask = ((crc & 1) as u32).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
            j += 1;
        }
        
        table[i as usize] = crc;
        i += 1;
    }
    
    table
}

/// Checksum simple XOR (très rapide mais peu robuste)
#[allow(dead_code)]
pub fn checksum_xor(data: &[u8]) -> u32 {
    let mut checksum = 0u32;
    
    // Traiter par chunks de 4 bytes
    let chunks = data.chunks_exact(4);
    let remainder = chunks.remainder();
    
    for chunk in chunks {
        let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        checksum ^= value;
    }
    
    // Traiter les bytes restants
    for (i, &byte) in remainder.iter().enumerate() {
        checksum ^= (byte as u32) << (i * 8);
    }
    
    checksum
}

/// Adler-32 checksum (compromis vitesse/qualité)
#[allow(dead_code)]
pub fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    
    let mut a = 1u32;
    let mut b = 0u32;
    
    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    
    (b << 16) | a
}

/// Vérifie si le CPU supporte CRC32C en hardware (SSE4.2)
#[cfg(target_arch = "x86_64")]
pub fn has_hardware_crc32c() -> bool {
    #[cfg(target_feature = "sse4.2")]
    {
        true
    }
    #[cfg(not(target_feature = "sse4.2"))]
    {
        // Pourrait utiliser CPUID pour détecter dynamiquement
        false
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn has_hardware_crc32c() -> bool {
    false
}

/// Calcule un checksum 64-bit en combinant CRC32C et longueur
pub fn checksum64(data: &[u8]) -> u64 {
    let crc = crc32c(data) as u64;
    let len = data.len() as u64;
    (crc << 32) | (len & 0xFFFFFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_crc32c_empty() {
        let data = b"";
        let crc = crc32c(data);
        assert_eq!(crc, 0);
    }
    
    #[test]
    fn test_crc32c_known_value() {
        // Test avec une valeur connue
        let data = b"123456789";
        let crc = crc32c(data);
        // CRC32C de "123456789" devrait être 0xE3069283
        assert_eq!(crc, 0xE3069283);
    }
    
    #[test]
    fn test_crc32c_different_data() {
        let data1 = b"hello";
        let data2 = b"world";
        let crc1 = crc32c(data1);
        let crc2 = crc32c(data2);
        assert_ne!(crc1, crc2);
    }
    
    #[test]
    fn test_checksum_xor() {
        let data = b"test data";
        let checksum = checksum_xor(data);
        assert_ne!(checksum, 0);
    }
    
    #[test]
    fn test_adler32() {
        let data = b"Wikipedia";
        let checksum = adler32(data);
        // Adler32 de "Wikipedia" devrait être 0x11E60398
        assert_eq!(checksum, 0x11E60398);
    }
    
    #[test]
    fn test_checksum64() {
        let data = b"test";
        let checksum = checksum64(data);
        assert_ne!(checksum, 0);
        
        // Vérifier que la longueur est encodée
        let len = (checksum & 0xFFFFFFFF) as usize;
        assert_eq!(len, data.len());
    }
}
