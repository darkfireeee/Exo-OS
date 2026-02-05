// libs/exo_crypto/src/lib.rs
#![no_std]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

pub mod kyber;
pub mod dilithium;
pub mod chacha20;

// Réexportations
pub use kyber::{kyber_keypair, kyber_encaps, kyber_decaps, KYBER_PUBLICKEYBYTES, KYBER_SECRETKEYBYTES, KYBER_CIPHERTEXTBYTES, KYBER_BYTES};
pub use dilithium::{dilithium_keypair, dilithium_sign, dilithium_verify, DILITHIUM_PUBLICKEYBYTES, DILITHIUM_SECRETKEYBYTES, DILITHIUM_BYTES};
pub use chacha20::{ChaCha20, XChaCha20, POLY1305_KEYBYTES, POLY1305_TAGBYTES};

/// Fonction de hash rapide pour les opérations internes
/// ATTENTION: Pas pour une utilisation cryptographique directe
pub fn fast_hash(data: &[u8]) -> u64 {
    // Djb2 hash modifié pour être plus rapide
    let mut hash: u64 = 5381;
    
    for byte in data {
        hash = ((hash << 5) + hash) ^ (*byte as u64);
    }
    
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fast_hash() {
        let data1 = b"hello";
        let data2 = b"world";
        let data3 = b"hello";
        
        let h1 = fast_hash(data1);
        let h2 = fast_hash(data2);
        let h3 = fast_hash(data3);
        
        assert_ne!(h1, h2);
        assert_eq!(h1, h3);
    }
}