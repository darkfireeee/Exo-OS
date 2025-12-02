//! ChaCha20-Poly1305 AEAD
//!
//! RFC 8439 Authenticated Encryption

use super::chacha20::ChaCha20;
use super::poly1305::Poly1305;
use alloc::vec::Vec;

pub struct ChaCha20Poly1305 {
    key: [u8; 32],
}

impl ChaCha20Poly1305 {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { key: *key }
    }

    fn pad16(len: usize) -> usize {
        (16 - (len % 16)) % 16
    }

    pub fn encrypt(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        // Poly1305 key from block 0
        let mut poly_key = [0u8; 32];
        let mut cipher = ChaCha20::new(&self.key, nonce, 0);
        cipher.encrypt(&mut poly_key);

        // Encrypt with block 1+
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = ChaCha20::new(&self.key, nonce, 1);
        cipher.encrypt(&mut ciphertext);

        // Compute MAC
        let mut mac = Poly1305::new(&poly_key);
        mac.update(aad);
        let pad = alloc::vec![0u8; Self::pad16(aad.len())];
        mac.update(&pad);
        mac.update(&ciphertext);
        let pad = alloc::vec![0u8; Self::pad16(ciphertext.len())];
        mac.update(&pad);
        let mut lengths = [0u8; 16];
        lengths[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        lengths[8..].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
        mac.update(&lengths);

        let tag = mac.finalize();
        ciphertext.extend_from_slice(&tag);
        ciphertext
    }

    pub fn decrypt(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        if data.len() < 16 {
            return Err("Too short");
        }

        let (ciphertext, tag) = data.split_at(data.len() - 16);

        // Poly1305 key
        let mut poly_key = [0u8; 32];
        let mut cipher = ChaCha20::new(&self.key, nonce, 0);
        cipher.encrypt(&mut poly_key);

        // Verify MAC
        let mut mac = Poly1305::new(&poly_key);
        mac.update(aad);
        let pad = alloc::vec![0u8; Self::pad16(aad.len())];
        mac.update(&pad);
        mac.update(ciphertext);
        let pad = alloc::vec![0u8; Self::pad16(ciphertext.len())];
        mac.update(&pad);
        let mut lengths = [0u8; 16];
        lengths[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        lengths[8..].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
        mac.update(&lengths);

        let computed = mac.finalize();
        let mut diff = 0u8;
        for i in 0..16 {
            diff |= tag[i] ^ computed[i];
        }
        if diff != 0 {
            return Err("Auth failed");
        }

        // Decrypt
        let mut plaintext = ciphertext.to_vec();
        let mut cipher = ChaCha20::new(&self.key, nonce, 1);
        cipher.decrypt(&mut plaintext);
        Ok(plaintext)
    }
}

pub fn encrypt_aead(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    ChaCha20Poly1305::new(key).encrypt(nonce, aad, plaintext)
}

pub fn decrypt_aead(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, &'static str> {
    ChaCha20Poly1305::new(key).decrypt(nonce, aad, ciphertext)
}
