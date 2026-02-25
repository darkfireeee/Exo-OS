// kernel/src/security/crypto/xchacha20_poly1305.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// XCHACHA20-POLY1305 — AEAD (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation complète de XChaCha20-Poly1305 AEAD.
// Utilisé pour TOUS les canaux kernel chiffrés (IPC sécurisé, crypto keys…).
//
// STRUCTURE :
//   • HChaCha20 : dérive une sous-clé de 256 bits depuis la clé + les 16 premiers
//     bytes du nonce XChaCha20 (nonce étendu 192 bits)
//   • ChaCha20  : chiffrement par flux avec la sous-clé + nonce résiduel (8 bytes)
//   • Poly1305  : MAC sur AAD + ciphertext (tag 16 bytes)
//
// RÉFÉRENCE : RFC 8439 (ChaCha20-Poly1305) + draft-irtf-cfrg-xchacha
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

// ─────────────────────────────────────────────────────────────────────────────
// ChaCha20 — bloc de keystream
// ─────────────────────────────────────────────────────────────────────────────

/// Constantes ChaCha20 ("expand 32-byte k").
const CHACHA20_CONSTANTS: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

#[inline(always)]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

/// Génère un bloc ChaCha20 de 64 bytes.
fn chacha20_block(key: &[u32; 8], counter: u32, nonce: &[u32; 3]) -> [u8; 64] {
    let mut state = [
        CHACHA20_CONSTANTS[0], CHACHA20_CONSTANTS[1],
        CHACHA20_CONSTANTS[2], CHACHA20_CONSTANTS[3],
        key[0], key[1], key[2], key[3],
        key[4], key[5], key[6], key[7],
        counter, nonce[0], nonce[1], nonce[2],
    ];
    let initial = state;

    for _ in 0..10 {
        // Colonnes
        quarter_round(&mut state, 0,  4,  8, 12);
        quarter_round(&mut state, 1,  5,  9, 13);
        quarter_round(&mut state, 2,  6, 10, 14);
        quarter_round(&mut state, 3,  7, 11, 15);
        // Diagonales
        quarter_round(&mut state, 0,  5, 10, 15);
        quarter_round(&mut state, 1,  6, 11, 12);
        quarter_round(&mut state, 2,  7,  8, 13);
        quarter_round(&mut state, 3,  4,  9, 14);
    }

    for (s, i) in state.iter_mut().zip(initial.iter()) {
        *s = s.wrapping_add(*i);
    }

    let mut out = [0u8; 64];
    for (i, word) in state.iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// HChaCha20 — dérive une sous-clé 256 bits depuis (key, nonce[0..15]).
fn hchacha20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32] {
    let mut key_words = [0u32; 8];
    for (i, w) in key_words.iter_mut().enumerate() {
        *w = u32::from_le_bytes(key[i*4..i*4+4].try_into().unwrap());
    }
    let n0 = u32::from_le_bytes(nonce[0..4].try_into().unwrap());
    let n1 = u32::from_le_bytes(nonce[4..8].try_into().unwrap());
    let n2 = u32::from_le_bytes(nonce[8..12].try_into().unwrap());
    let n3 = u32::from_le_bytes(nonce[12..16].try_into().unwrap());

    let mut state = [
        CHACHA20_CONSTANTS[0], CHACHA20_CONSTANTS[1],
        CHACHA20_CONSTANTS[2], CHACHA20_CONSTANTS[3],
        key_words[0], key_words[1], key_words[2], key_words[3],
        key_words[4], key_words[5], key_words[6], key_words[7],
        n0, n1, n2, n3,
    ];

    for _ in 0..10 {
        quarter_round(&mut state, 0,  4,  8, 12);
        quarter_round(&mut state, 1,  5,  9, 13);
        quarter_round(&mut state, 2,  6, 10, 14);
        quarter_round(&mut state, 3,  7, 11, 15);
        quarter_round(&mut state, 0,  5, 10, 15);
        quarter_round(&mut state, 1,  6, 11, 12);
        quarter_round(&mut state, 2,  7,  8, 13);
        quarter_round(&mut state, 3,  4,  9, 14);
    }

    let mut out = [0u8; 32];
    // HChaCha20 : prend les mots 0..3 et 12..15 (pas l'addition finale)
    for (i, &w) in [state[0],state[1],state[2],state[3]].iter().enumerate() {
        out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    for (i, &w) in [state[12],state[13],state[14],state[15]].iter().enumerate() {
        out[16+i*4..16+i*4+4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Chiffre/déchiffre un buffer avec ChaCha20 (modifie en place).
fn chacha20_xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], buf: &mut [u8]) {
    let mut key_words = [0u32; 8];
    for (i, w) in key_words.iter_mut().enumerate() {
        *w = u32::from_le_bytes(key[i*4..i*4+4].try_into().unwrap());
    }
    let nonce_words = [
        u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce[8..12].try_into().unwrap()),
    ];

    let mut pos = 0;
    let mut blk_counter = counter;
    while pos < buf.len() {
        let keystream = chacha20_block(&key_words, blk_counter, &nonce_words);
        let take = (buf.len() - pos).min(64);
        for i in 0..take {
            buf[pos + i] ^= keystream[i];
        }
        pos += 64;
        blk_counter += 1;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Poly1305 — MAC
// ─────────────────────────────────────────────────────────────────────────────

// Arithmetic modulo 2^130-5 using 5 × u32 limbs (radix 2^26).
struct Poly1305 {
    r:    [u32; 5],
    s:    [u32; 4],
    h:    [u32; 5],
    leftover: [u8; 16],
    leftover_len: usize,
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        // r = key[0..15] (avec clamping Poly1305)
        let r0 = u32::from_le_bytes(key[0..4].try_into().unwrap());
        let r1 = u32::from_le_bytes(key[4..8].try_into().unwrap());
        let r2 = u32::from_le_bytes(key[8..12].try_into().unwrap());
        let r3 = u32::from_le_bytes(key[12..16].try_into().unwrap());

        // Clamping constants: clear bits selon RFC 8439
        let r = [
            (r0)       & 0x0FFFFFFF,
            (r1 >> 2)  & 0x0FFFFFFC,
            (r2 >> 4)  & 0x0FFFFFFC,
            (r3 >> 6)  & 0x0FFFFFFC,
            0,
        ];

        // s = key[16..31]
        let s = [
            u32::from_le_bytes(key[16..20].try_into().unwrap()),
            u32::from_le_bytes(key[20..24].try_into().unwrap()),
            u32::from_le_bytes(key[24..28].try_into().unwrap()),
            u32::from_le_bytes(key[28..32].try_into().unwrap()),
        ];

        Self { r, s, h: [0u32; 5], leftover: [0u8; 16], leftover_len: 0 }
    }

    fn block(&mut self, m: &[u8; 16], is_final: bool) {
        let hibit: u32 = if is_final { 0 } else { 1 << 24 };

        // Lire le bloc comme 4 mots + bit de finalisation
        let m0 = u32::from_le_bytes(m[0..4].try_into().unwrap());
        let m1 = u32::from_le_bytes(m[4..8].try_into().unwrap());
        let m2 = u32::from_le_bytes(m[8..12].try_into().unwrap());
        let m3 = u32::from_le_bytes(m[12..16].try_into().unwrap());

        // Ajouter le message dans h (h += m)
        let h0 = (self.h[0] as u64) + (m0 as u64 & 0x3FFFFFF);
        let h1 = (self.h[1] as u64) + ((m0 as u64 >> 26) | ((m1 as u64) << 6)) & 0x3FFFFFF;
        let h2 = (self.h[2] as u64) + ((m1 as u64 >> 20) | ((m2 as u64) << 12)) & 0x3FFFFFF;
        let h3 = (self.h[3] as u64) + ((m2 as u64 >> 14) | ((m3 as u64) << 18)) & 0x3FFFFFF;
        let h4 = (self.h[4] as u64) + (m3 as u64 >> 8) + hibit as u64;

        // Multiplier par r (modulo 2^130-5)
        let r0 = self.r[0] as u64;
        let r1 = self.r[1] as u64;
        let r2 = self.r[2] as u64;
        let r3 = self.r[3] as u64;

        // 5 × r[i] (pour la réduction mod p)
        let r1_5 = r1 * 5;
        let r2_5 = r2 * 5;
        let r3_5 = r3 * 5;

        let d0 = h0*r0 + h1*r3_5 + h2*r2_5 + h3*r1_5 + h4*(self.r[3] as u64 * 5 >> 2);
        let d1 = h0*r1 + h1*r0   + h2*r3_5  + h3*r2_5  + h4*r1_5;
        let d2 = h0*r2 + h1*r1   + h2*r0    + h3*r3_5   + h4*r2_5;
        let d3 = h0*r3 + h1*r2   + h2*r1    + h3*r0     + h4*r3_5;
        let d4 = h4;

        // Propagation des retenues
        let c: u64;
        let h0_new = d0 & 0x3FFFFFF; let c1 = d0 >> 26;
        let h1_new = (d1 + c1) & 0x3FFFFFF; let c2 = (d1 + c1) >> 26;
        let h2_new = (d2 + c2) & 0x3FFFFFF; let c3 = (d2 + c2) >> 26;
        let h3_new = (d3 + c3) & 0x3FFFFFF; c = (d3 + c3) >> 26;
        let h4_new = d4 + c;

        self.h = [h0_new as u32, h1_new as u32, h2_new as u32, h3_new as u32, h4_new as u32];
    }

    fn update(&mut self, mut data: &[u8]) {
        if self.leftover_len > 0 {
            let want = 16 - self.leftover_len;
            let take = want.min(data.len());
            self.leftover[self.leftover_len..self.leftover_len + take]
                .copy_from_slice(&data[..take]);
            self.leftover_len += take;
            data = &data[take..];
            if self.leftover_len == 16 {
                let block = self.leftover;
                self.block(&block, false);
                self.leftover_len = 0;
            } else {
                return;
            }
        }
        while data.len() >= 16 {
            let block: [u8; 16] = data[..16].try_into().unwrap();
            self.block(&block, false);
            data = &data[16..];
        }
        if !data.is_empty() {
            self.leftover[..data.len()].copy_from_slice(data);
            self.leftover_len = data.len();
        }
    }

    fn finalize(mut self) -> [u8; 16] {
        if self.leftover_len > 0 {
            let mut block = [0u8; 16];
            block[..self.leftover_len].copy_from_slice(&self.leftover[..self.leftover_len]);
            block[self.leftover_len] = 1;
            self.block(&block, true);
        }

        // Réduction finale mod 2^130-5
        let mut h = self.h;
        let c = h[4] >> 26;
        h[4] &= 0x3FFFFFF;
        h[0] += c * 5;
        let c2 = h[0] >> 26;
        h[0] &= 0x3FFFFFF;
        h[1] += c2;

        // Comparaison h vs p = 2^130-5
        let mut g = [0u32; 5];
        let mut c3 = 5u64;
        for i in 0..4 {
            c3 += h[i] as u64;
            g[i] = (c3 & 0x3FFFFFF) as u32;
            c3 >>= 26;
        }
        g[4] = (h[4] as u64 + c3 - (1 << 26)) as u32;
        // Sélectionner h ou g selon si h >= p
        let mask = !((g[4] >> 31).wrapping_sub(1));
        for i in 0..5 {
            h[i] = (h[i] & !mask) | (g[i] & mask);
        }

        // Convertir h en bytes et ajouter s
        let h0 = ((h[0]) | (h[1] << 26)) as u64;
        let h1 = ((h[1] >> 6) | (h[2] << 20)) as u64;
        let h2 = ((h[2] >> 12) | (h[3] << 14)) as u64;
        let h3 = ((h[3] >> 18) | (h[4] << 8)) as u64;

        let mut tag = [0u8; 16];
        let f0 = h0 + self.s[0] as u64;
        let f1 = h1 + self.s[1] as u64 + (f0 >> 32);
        let f2 = h2 + self.s[2] as u64 + (f1 >> 32);
        let f3 = h3 + self.s[3] as u64 + (f2 >> 32);
        tag[0..4].copy_from_slice(&(f0 as u32).to_le_bytes());
        tag[4..8].copy_from_slice(&(f1 as u32).to_le_bytes());
        tag[8..12].copy_from_slice(&(f2 as u32).to_le_bytes());
        tag[12..16].copy_from_slice(&(f3 as u32).to_le_bytes());
        tag
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XChaCha20-Poly1305 AEAD
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur AEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadError {
    /// Tag d'authentification invalide (falsification détectée).
    AuthenticationFailed,
    /// Buffer de sortie trop petit.
    OutputTooSmall,
    /// Message trop long (> 2^64 - 1 bytes).
    MessageTooLong,
}

/// Taille du tag d'authentification en bytes.
pub const TAG_LEN: usize = 16;

/// Taille du nonce XChaCha20 en bytes.
pub const XCHACHA20_NONCE_LEN: usize = 24;

/// Taille de la clé en bytes.
pub const KEY_LEN: usize = 32;

/// Chiffre un message avec XChaCha20-Poly1305.
///
/// # Paramètres
/// * `key`        — clé 256 bits
/// * `nonce`      — nonce 192 bits (XChaCha20 étendu)
/// * `plaintext`  — message en clair (modifié en place → ciphertext)
/// * `aad`        — données authentifiées additionnelles (non chiffrées)
/// * `tag_out`    — tag résultant à stocker avec le ciphertext
pub fn xchacha20_poly1305_seal(
    key:       &[u8; KEY_LEN],
    nonce:     &[u8; XCHACHA20_NONCE_LEN],
    plaintext: &mut [u8],
    aad:       &[u8],
    tag_out:   &mut [u8; TAG_LEN],
) {
    // 1. HChaCha20 : dériver la sous-clé depuis les 16 premiers bytes du nonce
    let subkey_bytes: [u8; 32] = hchacha20(key, nonce[0..16].try_into().unwrap());

    // 2. Nonce ChaCha20 résiduel : 0x00000000 || nonce[16..23]
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce[4..12].copy_from_slice(&nonce[16..24]);

    // 3. Générer la clé Poly1305 (premier bloc ChaCha20, counter=0)
    let mut poly_key_buf = [0u8; 64];
    poly_key_buf[..32].copy_from_slice(&subkey_bytes);
    chacha20_xor(&subkey_bytes, 0, &chacha_nonce, &mut poly_key_buf[..64]);
    // Utiliser uniquement les 32 premiers bytes comme clé Poly1305
    let poly_key: [u8; 32] = poly_key_buf[..32].try_into().unwrap();

    // 4. Chiffrer le plaintext (counter=1)
    chacha20_xor(&subkey_bytes, 1, &chacha_nonce, plaintext);

    // 5. Calculer le tag Poly1305 sur AAD || pad || ciphertext || pad || longueurs
    let mut mac = Poly1305::new(&poly_key);
    // AAD
    mac.update(aad);
    // Padding AAD jusqu'au prochain multiple de 16
    let aad_pad = (16 - aad.len() % 16) % 16;
    mac.update(&[0u8; 15][..aad_pad]);
    // Ciphertext
    mac.update(plaintext);
    let ct_pad = (16 - plaintext.len() % 16) % 16;
    mac.update(&[0u8; 15][..ct_pad]);
    // Longueurs (8+8 bytes LE)
    mac.update(&(aad.len() as u64).to_le_bytes());
    mac.update(&(plaintext.len() as u64).to_le_bytes());

    *tag_out = mac.finalize();
}

/// Déchiffre et vérifie un message XChaCha20-Poly1305.
///
/// Retourne `Ok(())` si le tag est valide, `Err(AuthenticationFailed)` sinon.
/// Le buffer `ciphertext` est modifié en place (devient le plaintext).
pub fn xchacha20_poly1305_open(
    key:        &[u8; KEY_LEN],
    nonce:      &[u8; XCHACHA20_NONCE_LEN],
    ciphertext: &mut [u8],
    aad:        &[u8],
    tag:        &[u8; TAG_LEN],
) -> Result<(), AeadError> {
    // Dérivation identique à seal
    let subkey_bytes: [u8; 32] = hchacha20(key, nonce[0..16].try_into().unwrap());
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce[4..12].copy_from_slice(&nonce[16..24]);

    let mut poly_key_buf = [0u8; 64];
    poly_key_buf[..32].copy_from_slice(&subkey_bytes);
    chacha20_xor(&subkey_bytes, 0, &chacha_nonce, &mut poly_key_buf[..64]);
    let poly_key: [u8; 32] = poly_key_buf[..32].try_into().unwrap();

    // Vérifier le tag AVANT de déchiffrer (authenticated decryption)
    let mut mac = Poly1305::new(&poly_key);
    mac.update(aad);
    let aad_pad = (16 - aad.len() % 16) % 16;
    mac.update(&[0u8; 15][..aad_pad]);
    mac.update(ciphertext);
    let ct_pad = (16 - ciphertext.len() % 16) % 16;
    mac.update(&[0u8; 15][..ct_pad]);
    mac.update(&(aad.len() as u64).to_le_bytes());
    mac.update(&(ciphertext.len() as u64).to_le_bytes());

    let computed_tag = mac.finalize();

    // Comparaison en temps constant
    let mut diff = 0u8;
    for (a, b) in computed_tag.iter().zip(tag.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return Err(AeadError::AuthenticationFailed);
    }

    // Déchiffrer
    chacha20_xor(&subkey_bytes, 1, &chacha_nonce, ciphertext);
    Ok(())
}
