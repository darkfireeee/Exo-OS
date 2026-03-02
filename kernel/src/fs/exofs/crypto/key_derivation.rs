//! Dérivation de clés pour ExoFS — HKDF simplifié sur Blake3 (no_std).
//!
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

/// Contexte de dérivation de clé.
#[derive(Debug, Clone)]
pub struct KeyDerivation {
    /// Pseudo-random key (PRK) issue de l'extract.
    pub prk: [u8; 32],
}

/// Clé dérivée (256-bit).
#[derive(Clone, Debug)]
pub struct DerivedKey {
    pub bytes: [u8; 32],
    pub info:  &'static str,
}

impl Drop for DerivedKey {
    fn drop(&mut self) {
        // Zeroize : efface le matériel clé de la mémoire.
        self.bytes.iter_mut().for_each(|b| *b = 0);
    }
}

impl KeyDerivation {
    /// HKDF-Extract : dérive un PRK depuis le matériel (`ikm`) et le sel.
    pub fn extract(salt: &[u8], ikm: &[u8]) -> Self {
        // Utilise Blake3 comme PRF : HMAC-Blake3(salt, ikm).
        let prk = blake3_hmac(salt, ikm);
        Self { prk }
    }

    /// HKDF-Expand : produit `length` bytes pour un contexte `info` donné.
    pub fn expand(&self, info: &[u8], length: usize) -> Result<Vec<u8>, FsError> {
        if length == 0 || length > 255 * 32 {
            return Err(FsError::InvalidArgument);
        }

        let mut out = Vec::new();
        out.try_reserve(length).map_err(|_| FsError::OutOfMemory)?;

        let mut t = [0u8; 32];
        let mut counter: u8 = 0;
        let mut produced = 0;

        while produced < length {
            counter = counter.checked_add(1).ok_or(FsError::Overflow)?;
            // T(n) = HMAC-Blake3(PRK, T(n-1) || info || counter)
            let mut input = Vec::new();
            let cap = t.len() + info.len() + 1;
            input.try_reserve(cap).map_err(|_| FsError::OutOfMemory)?;
            if counter > 1 { input.extend_from_slice(&t); }
            input.extend_from_slice(info);
            input.push(counter);
            t = blake3_hmac(&self.prk, &input);

            let take = (length - produced).min(32);
            out.extend_from_slice(&t[..take]);
            produced += take;
        }

        Ok(out)
    }

    /// Dérive une clé de 256 bits avec un label statique.
    pub fn derive_256(&self, info: &'static str) -> Result<DerivedKey, FsError> {
        let raw = self.expand(info.as_bytes(), 32)?;
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&raw);
        Ok(DerivedKey { bytes, info })
    }

    /// Dérive une clé depuis un passphrase et un sel (hachage de mot de passe simplifié).
    pub fn from_passphrase(passphrase: &[u8], salt: &[u8; 32]) -> Self {
        // Itère 100_000 fois pour résistance brute-force.
        let mut state = blake3_hash_two(passphrase, salt);
        let iter_count = 100_000usize;
        for i in 0..iter_count {
            let ctr = (i as u64).to_le_bytes();
            state = blake3_hash_two(&state, &ctr);
        }
        Self { prk: state }
    }

    /// Dérive une ObjectKey depuis une VolumeKey et un BlobId.
    pub fn derive_object_key(
        volume_key: &[u8; 32],
        blob_id: &crate::fs::exofs::core::BlobId,
    ) -> DerivedKey {
        // Context = b"ExoFS.ObjectKey" || BlobId.bytes
        let mut ctx = [0u8; 48];
        ctx[..16].copy_from_slice(b"ExoFS.ObjectKey\x00");
        ctx[16..48].copy_from_slice(&blob_id.as_bytes());
        let bytes = blake3_hmac(volume_key, &ctx);
        DerivedKey { bytes, info: "ExoFS.ObjectKey" }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitives Blake3 HMAC (construction HMAC générique sur Blake3)
// ──────────────────────────────────────────────────────────────────────────────

/// HMAC simplifié utilisant Blake3 : H(key || H(key || data)).
fn blake3_hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    // Ipad / opad XOR.
    let mut k = [0u8; 32];
    let klen = key.len().min(32);
    k[..klen].copy_from_slice(&key[..klen]);

    let mut ipad = [0x36u8; 32];
    let mut opad = [0x5Cu8; 32];
    for i in 0..32 { ipad[i] ^= k[i]; opad[i] ^= k[i]; }

    let inner_len = 32 + data.len();
    let mut inner_input = alloc::vec::Vec::new();
    let _ = inner_input.try_reserve(inner_len);
    inner_input.extend_from_slice(&ipad);
    inner_input.extend_from_slice(data);
    let inner_hash = blake3_hash_slice(&inner_input);

    let mut outer_input = [0u8; 64];
    outer_input[..32].copy_from_slice(&opad);
    outer_input[32..].copy_from_slice(&inner_hash);
    blake3_hash_slice(&outer_input)
}

fn blake3_hash_two(a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut v = alloc::vec::Vec::new();
    let _ = v.try_reserve(a.len() + b.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    blake3_hash_slice(&v)
}

pub(super) fn blake3_hash_slice(data: &[u8]) -> [u8; 32] {
    // Utilise la même implémentation que entropy::blake3_hash adaptée à des entrées variables.
    const IV: [u32; 8] = [
        0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
        0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
    ];

    // Traitement de la première tranche (64 bytes).
    let mut msg = [0u8; 64];
    let n = data.len().min(64);
    msg[..n].copy_from_slice(&data[..n]);
    let words = bytes_to_words(&msg);
    let flags: u32 = (1 << 0) | (1 << 1) | (1 << 3); // CHUNK_START | CHUNK_END | ROOT
    let cv = blake3_compress_simple(&IV, &words, 0, n as u32, flags);
    let mut out = [0u8; 32];
    for (i, &w) in cv.iter().take(8).enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn bytes_to_words(b: &[u8; 64]) -> [u32; 16] {
    let mut w = [0u32; 16];
    for i in 0..16 {
        w[i] = u32::from_le_bytes(b[i*4..i*4+4].try_into().unwrap());
    }
    w
}

fn blake3_compress_simple(cv: &[u32; 8], msg: &[u32; 16], ctr: u64, blen: u32, flags: u32) -> [u32; 16] {
    let iv = [0x6A09E667u32, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A];
    let mut v: [u32; 16] = [
        cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7],
        iv[0], iv[1], iv[2], iv[3],
        (ctr & 0xFFFF_FFFF) as u32, (ctr >> 32) as u32, blen, flags,
    ];
    const SIGMA: [[usize; 16]; 7] = [
        [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],
        [2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8],
        [3,4,10,12,13,2,7,14,6,5,9,0,11,15,8,1],
        [10,7,12,9,14,3,13,15,4,0,11,2,5,8,1,6],
        [12,13,9,11,15,10,14,8,7,2,5,4,0,1,6,3],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
        [14,15,11,5,8,12,9,2,13,3,0,7,1,6,4,10],
    ];
    let mut g_fn = |a: usize, b: usize, c: usize, d: usize, x: u32, y: u32| {
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(x); v[d] = (v[d]^v[a]).rotate_right(16);
        v[c] = v[c].wrapping_add(v[d]);                 v[b] = (v[b]^v[c]).rotate_right(12);
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(y); v[d] = (v[d]^v[a]).rotate_right(8);
        v[c] = v[c].wrapping_add(v[d]);                 v[b] = (v[b]^v[c]).rotate_right(7);
    };
    for r in 0..7 {
        let s = &SIGMA[r];
        g_fn(0,4, 8,12,msg[s[0]],msg[s[1]]); g_fn(1,5, 9,13,msg[s[2]],msg[s[3]]);
        g_fn(2,6,10,14,msg[s[4]],msg[s[5]]); g_fn(3,7,11,15,msg[s[6]],msg[s[7]]);
        g_fn(0,5,10,15,msg[s[8]],msg[s[9]]); g_fn(1,6,11,12,msg[s[10]],msg[s[11]]);
        g_fn(2,7, 8,13,msg[s[12]],msg[s[13]]); g_fn(3,4, 9,14,msg[s[14]],msg[s[15]]);
    }
    for i in 0..8 { v[i] ^= v[i+8]; v[i+8] ^= cv[i]; }
    v
}
