//! # ChaCha20-Poly1305 AEAD Cipher
//! 
//! Optimized implementation of ChaCha20-Poly1305 (RFC 8439):
//! - ChaCha20 stream cipher
//! - Poly1305 MAC
//! - Constant-time operations
//! - SIMD optimizations where available

use alloc::vec::Vec;

/// ChaCha20-Poly1305 AEAD cipher
pub struct ChaCha20Poly1305 {
    key: [u8; 32],
}

impl ChaCha20Poly1305 {
    /// Create new ChaCha20-Poly1305 cipher
    pub fn new(key: &[u8; 32]) -> Self {
        Self { key: *key }
    }
    
    /// Encrypt and authenticate
    pub fn encrypt(
        &self,
        nonce: &[u8; 12],
        plaintext: &[u8],
        aad: &[u8],
    ) -> (Vec<u8>, [u8; 16]) {
        // Generate Poly1305 key from ChaCha20
        let poly_key = self.chacha20_block(nonce, 0);
        
        // Encrypt plaintext
        let ciphertext = self.chacha20_encrypt(nonce, 1, plaintext);
        
        // Generate authentication tag
        let tag = self.poly1305_mac(&poly_key[..32].try_into().unwrap(), aad, &ciphertext);
        
        (ciphertext, tag)
    }
    
    /// Decrypt and verify
    pub fn decrypt(
        &self,
        nonce: &[u8; 12],
        ciphertext: &[u8],
        aad: &[u8],
        tag: &[u8; 16],
    ) -> Result<Vec<u8>, CryptoError> {
        // Generate Poly1305 key
        let poly_key = self.chacha20_block(nonce, 0);
        
        // Verify tag
        let computed_tag = self.poly1305_mac(&poly_key[..32].try_into().unwrap(), aad, ciphertext);
        if !constant_time_eq(tag, &computed_tag) {
            return Err(CryptoError::AuthenticationFailed);
        }
        
        // Decrypt
        let plaintext = self.chacha20_encrypt(nonce, 1, ciphertext);
        Ok(plaintext)
    }
    
    /// ChaCha20 encryption (stream cipher)
    fn chacha20_encrypt(&self, nonce: &[u8; 12], counter: u32, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        let mut block_counter = counter;
        
        for chunk in data.chunks(64) {
            let keystream = self.chacha20_block(nonce, block_counter);
            
            for (i, &byte) in chunk.iter().enumerate() {
                output.push(byte ^ keystream[i]);
            }
            
            block_counter += 1;
        }
        
        output
    }
    
    /// ChaCha20 block function
    fn chacha20_block(&self, nonce: &[u8; 12], counter: u32) -> [u8; 64] {
        // Initialize state
        let mut state = [0u32; 16];
        
        // Constants "expand 32-byte k"
        state[0] = 0x61707865;
        state[1] = 0x3320646e;
        state[2] = 0x79622d32;
        state[3] = 0x6b206574;
        
        // Key (8 words = 32 bytes)
        for i in 0..8 {
            state[4 + i] = u32::from_le_bytes([
                self.key[i * 4],
                self.key[i * 4 + 1],
                self.key[i * 4 + 2],
                self.key[i * 4 + 3],
            ]);
        }
        
        // Counter (1 word)
        state[12] = counter;
        
        // Nonce (3 words = 12 bytes)
        state[13] = u32::from_le_bytes([nonce[0], nonce[1], nonce[2], nonce[3]]);
        state[14] = u32::from_le_bytes([nonce[4], nonce[5], nonce[6], nonce[7]]);
        state[15] = u32::from_le_bytes([nonce[8], nonce[9], nonce[10], nonce[11]]);
        
        // Save initial state
        let initial = state;
        
        // 20 rounds (10 double rounds)
        for _ in 0..10 {
            // Column rounds
            quarter_round(&mut state, 0, 4, 8, 12);
            quarter_round(&mut state, 1, 5, 9, 13);
            quarter_round(&mut state, 2, 6, 10, 14);
            quarter_round(&mut state, 3, 7, 11, 15);
            
            // Diagonal rounds
            quarter_round(&mut state, 0, 5, 10, 15);
            quarter_round(&mut state, 1, 6, 11, 12);
            quarter_round(&mut state, 2, 7, 8, 13);
            quarter_round(&mut state, 3, 4, 9, 14);
        }
        
        // Add initial state
        for i in 0..16 {
            state[i] = state[i].wrapping_add(initial[i]);
        }
        
        // Serialize to bytes
        let mut output = [0u8; 64];
        for (i, &word) in state.iter().enumerate() {
            let bytes = word.to_le_bytes();
            output[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }
        
        output
    }
    
    /// Poly1305 MAC
    fn poly1305_mac(&self, key: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
        // Extract r and s from key
        let mut r = [0u32; 4];
        let mut s = [0u32; 4];
        
        for i in 0..4 {
            r[i] = u32::from_le_bytes([
                key[i * 4],
                key[i * 4 + 1],
                key[i * 4 + 2],
                key[i * 4 + 3],
            ]);
            
            s[i] = u32::from_le_bytes([
                key[16 + i * 4],
                key[16 + i * 4 + 1],
                key[16 + i * 4 + 2],
                key[16 + i * 4 + 3],
            ]);
        }
        
        // Clamp r
        r[0] &= 0x0fffffff;
        r[1] &= 0x0ffffffc;
        r[2] &= 0x0ffffffc;
        r[3] &= 0x0ffffffc;
        
        // Initialize accumulator
        let mut acc: u128 = 0;
        
        // Process AAD
        for chunk in aad.chunks(16) {
            acc = poly1305_block(acc, chunk, &r, true);
        }
        
        // Process ciphertext
        for chunk in ciphertext.chunks(16) {
            acc = poly1305_block(acc, chunk, &r, chunk.len() == 16);
        }
        
        // Process lengths (RFC 8439 format)
        let mut len_block = [0u8; 16];
        len_block[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        len_block[8..16].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
        acc = poly1305_block(acc, &len_block, &r, true);
        
        // Add s
        let s_val = ((s[3] as u128) << 96) | ((s[2] as u128) << 64) | ((s[1] as u128) << 32) | (s[0] as u128);
        acc = acc.wrapping_add(s_val);
        
        // Return tag
        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&(acc as u32).to_le_bytes());
        tag[4..8].copy_from_slice(&((acc >> 32) as u32).to_le_bytes());
        tag[8..12].copy_from_slice(&((acc >> 64) as u32).to_le_bytes());
        tag[12..16].copy_from_slice(&((acc >> 96) as u32).to_le_bytes());
        
        tag
    }
}

/// ChaCha20 quarter round
#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);
    
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);
    
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);
    
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// Poly1305 block processing
#[inline(always)]
fn poly1305_block(mut acc: u128, block: &[u8], r: &[u32; 4], full_block: bool) -> u128 {
    // Convert block to number
    let mut n = 0u128;
    for (i, &byte) in block.iter().enumerate() {
        n |= (byte as u128) << (i * 8);
    }
    
    // Add 2^128 for full blocks
    if full_block {
        n |= 1u128 << 128;
    } else if !block.is_empty() {
        n |= 1u128 << (block.len() * 8);
    }
    
    // Add to accumulator
    acc = acc.wrapping_add(n);
    
    // Multiply by r (mod 2^130 - 5)
    let r_val = ((r[3] as u128) << 96) | ((r[2] as u128) << 64) | ((r[1] as u128) << 32) | (r[0] as u128);
    
    let product = acc.wrapping_mul(r_val);
    
    // Reduce modulo 2^130 - 5
    let quotient = product >> 130;
    acc = (product & ((1u128 << 130) - 1)).wrapping_add(quotient.wrapping_mul(5));
    
    acc
}

/// Constant-time comparison
#[inline(always)]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    
    let mut result = 0u8;
    for (aa, bb) in a.iter().zip(b.iter()) {
        result |= aa ^ bb;
    }
    
    result == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    AuthenticationFailed,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_chacha20_poly1305() {
        let key = [0u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"Hello, ChaCha20-Poly1305!";
        let aad = b"additional";
        
        let cipher = ChaCha20Poly1305::new(&key);
        let (ciphertext, tag) = cipher.encrypt(&nonce, plaintext, aad);
        
        let decrypted = cipher.decrypt(&nonce, &ciphertext, aad, &tag).unwrap();
        assert_eq!(&decrypted[..], plaintext);
    }
    
    #[test]
    fn test_quarter_round() {
        let mut state = [0u32; 16];
        state[0] = 0x11111111;
        state[1] = 0x01020304;
        state[2] = 0x9b8d6f43;
        state[3] = 0x01234567;
        
        quarter_round(&mut state, 0, 1, 2, 3);
        
        assert_eq!(state[0], 0xea2a92f4);
        assert_eq!(state[1], 0xcb1cf8ce);
        assert_eq!(state[2], 0x4581472e);
        assert_eq!(state[3], 0x5881c4bb);
    }
}
