//! AES Encryption

use alloc::vec::Vec;

/// AES key
pub struct AesKey {
    pub key: Vec<u8>,
    pub bits: usize, // 128, 192, or 256
}

impl AesKey {
    pub fn new_128(key: &[u8; 16]) -> Self {
        Self {
            key: key.to_vec(),
            bits: 128,
        }
    }

    pub fn new_256(key: &[u8; 32]) -> Self {
        Self {
            key: key.to_vec(),
            bits: 256,
        }
    }
}

/// AES-GCM encrypt
pub fn aes_encrypt(key: &AesKey, nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
    // Stub - use external crypto library
    Err("AES not implemented")
}

/// AES-GCM decrypt
pub fn aes_decrypt(key: &AesKey, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, &'static str> {
    // Stub
    Err("AES not implemented")
}
