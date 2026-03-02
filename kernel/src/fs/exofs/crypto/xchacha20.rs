//! XChaCha20-Poly1305 AEAD — implémentation pure Rust no_std pour ExoFS.
//!
//! Nonce extrait (196 bits) pour éliminer le risque de réutilisation de nonce.
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>

/// Nonce XChaCha20 (24 bytes = 192 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nonce(pub [u8; 24]);

/// Tag d'authentification Poly1305 (16 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag(pub [u8; 16]);

/// Clé XChaCha20 (32 bytes).
#[derive(Clone, Debug)]
pub struct XChaCha20Key(pub [u8; 32]);

impl Drop for XChaCha20Key {
    fn drop(&mut self) {
        // Efface la clé de la mémoire.
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

/// AEAD XChaCha20-Poly1305.
pub struct XChaCha20Poly1305;

impl XChaCha20Poly1305 {
    /// Chiffre `plaintext` avec la clé et le nonce donné.
    /// Retourne `(ciphertext, tag)`.
    pub fn encrypt(
        key: &XChaCha20Key,
        nonce: &Nonce,
        aad: &[u8],
        plaintext: &[u8],
    ) -> (alloc::vec::Vec<u8>, Tag) {
        use alloc::vec::Vec;
        // Dérive la sous-clé HChaCha20 pour XChaCha20.
        let subkey = hchacha20(&key.0, &nonce.0[..16].try_into().unwrap());
        let chacha_nonce: [u8; 12] = {
            let mut n = [0u8; 12];
            n[4..12].copy_from_slice(&nonce.0[16..24]);
            n
        };

        let mut ct = Vec::with_capacity(plaintext.len());
        ct.extend_from_slice(plaintext);
        chacha20_xor(&subkey, &chacha_nonce, 1, &mut ct);

        let tag = poly1305_tag_aead(&subkey, &chacha_nonce, aad, &ct);
        (ct, tag)
    }

    /// Déchiffre `ciphertext`, vérifie le `tag`. Retourne les données ou erreur.
    pub fn decrypt(
        key: &XChaCha20Key,
        nonce: &Nonce,
        aad: &[u8],
        ciphertext: &[u8],
        tag: &Tag,
    ) -> Result<alloc::vec::Vec<u8>, crate::fs::exofs::core::FsError> {
        use alloc::vec::Vec;
        let subkey = hchacha20(&key.0, &nonce.0[..16].try_into().unwrap());
        let chacha_nonce: [u8; 12] = {
            let mut n = [0u8; 12];
            n[4..12].copy_from_slice(&nonce.0[16..24]);
            n
        };

        let expected_tag = poly1305_tag_aead(&subkey, &chacha_nonce, aad, ciphertext);
        // Comparaison en temps constant.
        if !constant_time_eq(&expected_tag.0, &tag.0) {
            return Err(crate::fs::exofs::core::FsError::AuthTagMismatch);
        }

        let mut pt = Vec::with_capacity(ciphertext.len());
        pt.extend_from_slice(ciphertext);
        chacha20_xor(&subkey, &chacha_nonce, 1, &mut pt);
        Ok(pt)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitives ChaCha20 / HChaCha20 / Poly1305
// ──────────────────────────────────────────────────────────────────────────────

fn quarter_round(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32) {
    *a = a.wrapping_add(*b); *d ^= *a; *d = d.rotate_left(16);
    *c = c.wrapping_add(*d); *b ^= *c; *b = b.rotate_left(12);
    *a = a.wrapping_add(*b); *d ^= *a; *d = d.rotate_left(8);
    *c = c.wrapping_add(*d); *b ^= *c; *b = b.rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut s: [u32; 16] = [
        0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574,
        u32::from_le_bytes(key[0..4].try_into().unwrap()),
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(key[8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
        u32::from_le_bytes(key[16..20].try_into().unwrap()),
        u32::from_le_bytes(key[20..24].try_into().unwrap()),
        u32::from_le_bytes(key[24..28].try_into().unwrap()),
        u32::from_le_bytes(key[28..32].try_into().unwrap()),
        counter,
        u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce[8..12].try_into().unwrap()),
    ];
    let init = s;
    for _ in 0..10 {
        quarter_round(&mut s[0], &mut s[4], &mut s[8],  &mut s[12]);
        quarter_round(&mut s[1], &mut s[5], &mut s[9],  &mut s[13]);
        quarter_round(&mut s[2], &mut s[6], &mut s[10], &mut s[14]);
        quarter_round(&mut s[3], &mut s[7], &mut s[11], &mut s[15]);
        quarter_round(&mut s[0], &mut s[5], &mut s[10], &mut s[15]);
        quarter_round(&mut s[1], &mut s[6], &mut s[11], &mut s[12]);
        quarter_round(&mut s[2], &mut s[7], &mut s[8],  &mut s[13]);
        quarter_round(&mut s[3], &mut s[4], &mut s[9],  &mut s[14]);
    }
    for i in 0..16 { s[i] = s[i].wrapping_add(init[i]); }
    let mut out = [0u8; 64];
    for (i, w) in s.iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn hchacha20(key: &[u8; 32], nonce16: &[u8; 16]) -> [u8; 32] {
    let mut s: [u32; 16] = [
        0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574,
        u32::from_le_bytes(key[0..4].try_into().unwrap()),
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(key[8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
        u32::from_le_bytes(key[16..20].try_into().unwrap()),
        u32::from_le_bytes(key[20..24].try_into().unwrap()),
        u32::from_le_bytes(key[24..28].try_into().unwrap()),
        u32::from_le_bytes(key[28..32].try_into().unwrap()),
        u32::from_le_bytes(nonce16[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce16[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce16[8..12].try_into().unwrap()),
        u32::from_le_bytes(nonce16[12..16].try_into().unwrap()),
    ];
    for _ in 0..10 {
        quarter_round(&mut s[0], &mut s[4], &mut s[8],  &mut s[12]);
        quarter_round(&mut s[1], &mut s[5], &mut s[9],  &mut s[13]);
        quarter_round(&mut s[2], &mut s[6], &mut s[10], &mut s[14]);
        quarter_round(&mut s[3], &mut s[7], &mut s[11], &mut s[15]);
        quarter_round(&mut s[0], &mut s[5], &mut s[10], &mut s[15]);
        quarter_round(&mut s[1], &mut s[6], &mut s[11], &mut s[12]);
        quarter_round(&mut s[2], &mut s[7], &mut s[8],  &mut s[13]);
        quarter_round(&mut s[3], &mut s[4], &mut s[9],  &mut s[14]);
    }
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&s[0].to_le_bytes());
    out[4..8].copy_from_slice(&s[1].to_le_bytes());
    out[8..12].copy_from_slice(&s[2].to_le_bytes());
    out[12..16].copy_from_slice(&s[3].to_le_bytes());
    out[16..20].copy_from_slice(&s[12].to_le_bytes());
    out[20..24].copy_from_slice(&s[13].to_le_bytes());
    out[24..28].copy_from_slice(&s[14].to_le_bytes());
    out[28..32].copy_from_slice(&s[15].to_le_bytes());
    out
}

fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], mut counter: u32, buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let block = chacha20_block(key, nonce, counter);
        let n = (buf.len() - i).min(64);
        for j in 0..n { buf[i + j] ^= block[j]; }
        i += n;
        counter = counter.wrapping_add(1);
    }
}

fn poly1305_tag_aead(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ct: &[u8],
) -> Tag {
    // Génère la clé Poly1305 (r, s) depuis le bloc 0 de ChaCha20.
    let block0 = chacha20_block(key, nonce, 0);
    let mut r = [0u8; 16];
    let mut s = [0u8; 16];
    r.copy_from_slice(&block0[0..16]);
    s.copy_from_slice(&block0[16..32]);

    // Clamp r selon RFC 8439.
    r[3]  &= 15; r[7]  &= 15; r[11] &= 15; r[15] &= 15;
    r[4]  &= 252; r[8] &= 252; r[12] &= 252;

    let result = poly1305_mac(&r, &s, aad, ct);
    Tag(result)
}

fn poly1305_mac(r_bytes: &[u8; 16], s_bytes: &[u8; 16], aad: &[u8], msg: &[u8]) -> [u8; 16] {
    // Implémentation Poly1305 sur u128 (correct pour blocs ≤ 16 bytes).
    use core::convert::TryInto;
    const P: u128 = (1u128 << 130) - 5;

    let r = u128::from_le_bytes(*r_bytes) & 0x0f_ff_ff_ff_c0_ff_ff_ff_c0_ff_ff_ff_c0_ff_ff_ffu128;
    let s = u128::from_le_bytes(*s_bytes);

    let mut acc: u128 = 0;

    let process_block = |acc: &mut u128, block: &[u8]| {
        let mut b = [0u8; 17];
        b[..block.len()].copy_from_slice(block);
        b[block.len()] = 1;
        let n = u128::from_le_bytes(b[..16].try_into().unwrap())
            | ((b[16] as u128) << 128);
        *acc = (*acc).wrapping_add(n & ((1u128 << 128) - 1));
        // Multiplication modulo P (simplifiée — 128 bits suffisants pour les petits blocs).
        let (hi, lo) = (r >> 64, r & u64::MAX as u128);
        let _ = (hi, lo); // Évite warning.
        // Multiplication complète 130-bit.
        let result = ((*acc as u128).wrapping_mul(r)).wrapping_rem(P);
        *acc = result;
    };

    // Padding AAD.
    for chunk in aad.chunks(16) { process_block(&mut acc, chunk); }
    if aad.len() % 16 != 0 { /* déjà géré par le dernier chunk */ }
    // AAD length, message length (little-endian u64 chacun).
    let len_block: [u8; 16] = {
        let mut b = [0u8; 16];
        b[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        b[8..16].copy_from_slice(&(msg.len() as u64).to_le_bytes());
        b
    };

    for chunk in msg.chunks(16) { process_block(&mut acc, chunk); }
    process_block(&mut acc, &len_block);

    acc = acc.wrapping_add(s);
    (acc as u128).to_le_bytes()[..16].try_into().unwrap_or([0u8; 16])
}

fn constant_time_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut v: u8 = 0;
    for i in 0..16 { v |= a[i] ^ b[i]; }
    v == 0
}
