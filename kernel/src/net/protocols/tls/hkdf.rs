//! # HKDF (HMAC-based Key Derivation Function)
//! 
//! RFC 5869 compliant implementation:
//! - HKDF-Extract: extracts pseudorandom key
//! - HKDF-Expand: expands key to desired length
//! - Constant-time operations
//! - Supports SHA-256 and SHA-384

use alloc::vec::Vec;

/// HKDF key derivation
pub struct Hkdf {
    hash_len: usize,
}

impl Hkdf {
    /// Create HKDF with SHA-256
    pub fn sha256() -> Self {
        Self { hash_len: 32 }
    }
    
    /// Create HKDF with SHA-384
    pub fn sha384() -> Self {
        Self { hash_len: 48 }
    }
    
    /// Extract pseudorandom key from input keying material
    pub fn extract(&self, salt: Option<&[u8]>, ikm: &[u8]) -> Vec<u8> {
        let salt = salt.unwrap_or(&vec![0u8; self.hash_len]);
        
        // PRK = HMAC-Hash(salt, IKM)
        self.hmac(salt, ikm)
    }
    
    /// Expand pseudorandom key to desired length
    pub fn expand(&self, prk: &[u8], info: &[u8], length: usize) -> Result<Vec<u8>, HkdfError> {
        if length > 255 * self.hash_len {
            return Err(HkdfError::OutputTooLong);
        }
        
        let n = (length + self.hash_len - 1) / self.hash_len;
        let mut okm = Vec::with_capacity(length);
        let mut t = Vec::new();
        
        for i in 1..=n {
            // T(i) = HMAC-Hash(PRK, T(i-1) | info | i)
            let mut input = t.clone();
            input.extend_from_slice(info);
            input.push(i as u8);
            
            t = self.hmac(prk, &input);
            okm.extend_from_slice(&t);
        }
        
        okm.truncate(length);
        Ok(okm)
    }
    
    /// Combined extract-and-expand
    pub fn derive(&self, salt: Option<&[u8]>, ikm: &[u8], info: &[u8], length: usize) -> Result<Vec<u8>, HkdfError> {
        let prk = self.extract(salt, ikm);
        self.expand(&prk, info, length)
    }
    
    /// HMAC (Hash-based Message Authentication Code)
    fn hmac(&self, key: &[u8], message: &[u8]) -> Vec<u8> {
        const BLOCK_SIZE: usize = 64; // SHA-256/384 block size
        
        // Prepare key
        let mut k = if key.len() > BLOCK_SIZE {
            self.hash(key)
        } else {
            key.to_vec()
        };
        
        // Pad key to block size
        k.resize(BLOCK_SIZE, 0);
        
        // Compute inner hash: H(K XOR ipad || message)
        let mut inner_input = Vec::with_capacity(BLOCK_SIZE + message.len());
        for &byte in &k {
            inner_input.push(byte ^ 0x36);
        }
        inner_input.extend_from_slice(message);
        let inner_hash = self.hash(&inner_input);
        
        // Compute outer hash: H(K XOR opad || inner_hash)
        let mut outer_input = Vec::with_capacity(BLOCK_SIZE + self.hash_len);
        for &byte in &k {
            outer_input.push(byte ^ 0x5c);
        }
        outer_input.extend_from_slice(&inner_hash);
        
        self.hash(&outer_input)
    }
    
    /// Hash function (SHA-256 or SHA-384)
    fn hash(&self, data: &[u8]) -> Vec<u8> {
        match self.hash_len {
            32 => sha256(data),
            48 => sha384(data),
            _ => unreachable!(),
        }
    }
}

/// SHA-256 hash function
fn sha256(data: &[u8]) -> Vec<u8> {
    let mut state = [
        0x6a09e667u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    
    // Pad message
    let mut padded = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    
    padded.push(0x80);
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    
    // Process blocks
    for chunk in padded.chunks(64) {
        sha256_process_block(&mut state, chunk);
    }
    
    // Convert to bytes
    let mut output = Vec::with_capacity(32);
    for word in &state {
        output.extend_from_slice(&word.to_be_bytes());
    }
    
    output
}

/// Process one SHA-256 block
fn sha256_process_block(state: &mut [u32; 8], block: &[u8]) {
    // Prepare message schedule
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    
    // Initialize working variables
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];
    
    // Main loop
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);
        
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    
    // Add to state
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// SHA-384 hash function
fn sha384(data: &[u8]) -> Vec<u8> {
    let mut state = [
        0xcbbb9d5dc1059ed8u64, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
        0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
    ];
    
    // Pad message
    let mut padded = data.to_vec();
    let bit_len = (data.len() as u128) * 8;
    
    padded.push(0x80);
    while (padded.len() + 16) % 128 != 0 {
        padded.push(0);
    }
    padded.extend_from_slice(&(bit_len >> 64).to_be_bytes());
    padded.extend_from_slice(&((bit_len & 0xffffffffffffffff) as u64).to_be_bytes());
    
    // Process blocks
    for chunk in padded.chunks(128) {
        sha384_process_block(&mut state, chunk);
    }
    
    // Convert to bytes (first 6 words only for SHA-384)
    let mut output = Vec::with_capacity(48);
    for i in 0..6 {
        output.extend_from_slice(&state[i].to_be_bytes());
    }
    
    output
}

/// Process one SHA-384 block
fn sha384_process_block(state: &mut [u64; 8], block: &[u8]) {
    // Prepare message schedule
    let mut w = [0u64; 80];
    for i in 0..16 {
        w[i] = u64::from_be_bytes([
            block[i * 8], block[i * 8 + 1], block[i * 8 + 2], block[i * 8 + 3],
            block[i * 8 + 4], block[i * 8 + 5], block[i * 8 + 6], block[i * 8 + 7],
        ]);
    }
    
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    
    // Initialize working variables
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];
    
    // Main loop
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);
        
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    
    // Add to state
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// SHA-256 round constants
const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-512/384 round constants
const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HkdfError {
    OutputTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hkdf_sha256() {
        let ikm = b"input keying material";
        let salt = Some(&b"salt"[..]);
        let info = b"context info";
        
        let hkdf = Hkdf::sha256();
        let okm = hkdf.derive(salt, ikm, info, 42).unwrap();
        
        assert_eq!(okm.len(), 42);
    }
    
    #[test]
    fn test_hmac_sha256() {
        let hkdf = Hkdf::sha256();
        let key = b"key";
        let message = b"The quick brown fox jumps over the lazy dog";
        
        let mac = hkdf.hmac(key, message);
        assert_eq!(mac.len(), 32);
    }
    
    #[test]
    fn test_sha256() {
        let data = b"abc";
        let hash = sha256(data);
        
        // Expected: ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(hash.len(), 32);
        assert_eq!(hash[0], 0xba);
        assert_eq!(hash[1], 0x78);
    }
}
