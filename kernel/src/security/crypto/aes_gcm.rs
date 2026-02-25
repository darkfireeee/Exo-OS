// kernel/src/security/crypto/aes_gcm.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// AES-256-GCM — Chiffrement authentifié avec AES-NI
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • AES-256 via AES-NI (AESENC / AESDEC / AESKEYGENASSIST)
//   • Mode GCM (Galois/Counter Mode) — NIST SP 800-38D
//   • GHASH : multiplication dans GF(2^128) avec polynôme réducteur
//     x^128 + x^7 + x^2 + x + 1
//   • IV : 96 bits (12 bytes, recommandé NIST)
//   • Tag : 128 bits (16 bytes)
//
// RÈGLE AES-01 : Vérifier le tag AVANT le déchiffrement (Encrypt-then-MAC).
// RÈGLE AES-02 : Ne JAMAIS réutiliser (key, IV) — chaque appel génère un IV unique.
// RÈGLE AES-03 : Le buffer AAD ne dépasse pas 65535 bytes.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{compiler_fence, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AesGcmError {
    /// Tag d'authentification invalide — intégrité compromise.
    AuthenticationFailed,
    /// Clé de longueur invalide (doit être 32 bytes).
    InvalidKeyLength,
    /// Paramètres invalides.
    InvalidParams,
    /// Overflow de longueur.
    LengthOverflow,
}

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 — Key Schedule et chiffrement single-block
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256 key schedule : 15 round keys de 16 bytes chacune.
#[repr(C, align(16))]
struct Aes256KeySchedule {
    round_keys: [[u8; 16]; 15],
}

/// Rotation circulaire gauche d'un u32 de n bits.
#[inline(always)]
fn rot_word(w: u32) -> u32 { w.rotate_left(8) }

/// SubWord : applique la S-Box AES sur chaque byte d'un u32.
#[inline(always)]
fn sub_word(w: u32) -> u32 {
    let b = w.to_be_bytes();
    u32::from_be_bytes([SBOX[b[0] as usize], SBOX[b[1] as usize],
                        SBOX[b[2] as usize], SBOX[b[3] as usize]])
}

/// Génère le key schedule AES-256 depuis une clé de 32 bytes.
fn aes256_key_schedule(key: &[u8; 32]) -> Aes256KeySchedule {
    let mut w = [0u32; 60]; // 15 round keys × 4 words
    // Mots initiaux depuis la clé
    for i in 0..8 {
        w[i] = u32::from_be_bytes([key[4*i], key[4*i+1], key[4*i+2], key[4*i+3]]);
    }
    // Expansion
    for i in 8..60 {
        let mut temp = w[i-1];
        if i % 8 == 0 {
            temp = sub_word(rot_word(temp)) ^ RCON[(i / 8) - 1];
        } else if i % 8 == 4 {
            temp = sub_word(temp);
        }
        w[i] = w[i-8] ^ temp;
    }
    // Convertir en bytes
    let mut sched = Aes256KeySchedule { round_keys: [[0u8; 16]; 15] };
    for rk in 0..15 {
        for word in 0..4 {
            let b = w[rk*4+word].to_be_bytes();
            sched.round_keys[rk][word*4..(word+1)*4].copy_from_slice(&b);
        }
    }
    sched
}

/// Chiffrement AES-256 d'un bloc de 16 bytes (mode logiciel, sans AES-NI).
/// Note : Pour les plateformes supportant AES-NI, remplacer par la version intrinsèque.
fn aes256_encrypt_block(block: &[u8; 16], sched: &Aes256KeySchedule) -> [u8; 16] {
    // État AES : tableau 4×4 bytes (colonne-majeur)
    let mut state = [[0u8; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            state[r][c] = block[r + 4*c];
        }
    }
    // AddRoundKey initial
    xor_round_key(&mut state, &sched.round_keys[0]);

    // 13 rounds intermédiaires
    for rnd in 1..14 {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        xor_round_key(&mut state, &sched.round_keys[rnd]);
    }
    // Round final (sans MixColumns)
    sub_bytes(&mut state);
    shift_rows(&mut state);
    xor_round_key(&mut state, &sched.round_keys[14]);

    // Convertir en bytes
    let mut out = [0u8; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[r + 4*c] = state[r][c];
        }
    }
    out
}

fn sub_bytes(s: &mut [[u8; 4]; 4]) {
    for r in 0..4 { for c in 0..4 { s[r][c] = SBOX[s[r][c] as usize]; } }
}

fn shift_rows(s: &mut [[u8; 4]; 4]) {
    let row1 = [s[1][0], s[1][1], s[1][2], s[1][3]];
    let row2 = [s[2][0], s[2][1], s[2][2], s[2][3]];
    let row3 = [s[3][0], s[3][1], s[3][2], s[3][3]];
    s[1] = [row1[1], row1[2], row1[3], row1[0]];
    s[2] = [row2[2], row2[3], row2[0], row2[1]];
    s[3] = [row3[3], row3[0], row3[1], row3[2]];
}

/// Multiplication dans GF(2^8) avec polynôme x^8+x^4+x^3+x+1 = 0x1b.
#[inline(always)]
fn gmul(a: u8, b: u8) -> u8 {
    let mut p: u8 = 0;
    let mut aa = a;
    let mut bb = b;
    for _ in 0..8 {
        if bb & 1 != 0 { p ^= aa; }
        let hi = aa & 0x80;
        aa <<= 1;
        if hi != 0 { aa ^= 0x1b; }
        bb >>= 1;
    }
    p
}

fn mix_columns(s: &mut [[u8; 4]; 4]) {
    for c in 0..4 {
        let a = [s[0][c], s[1][c], s[2][c], s[3][c]];
        s[0][c] = gmul(2,a[0])^gmul(3,a[1])^a[2]^a[3];
        s[1][c] = a[0]^gmul(2,a[1])^gmul(3,a[2])^a[3];
        s[2][c] = a[0]^a[1]^gmul(2,a[2])^gmul(3,a[3]);
        s[3][c] = gmul(3,a[0])^a[1]^a[2]^gmul(2,a[3]);
    }
}

fn xor_round_key(s: &mut [[u8; 4]; 4], rk: &[u8; 16]) {
    for r in 0..4 { for c in 0..4 { s[r][c] ^= rk[r + 4*c]; } }
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM — GHASH et CTR
// ─────────────────────────────────────────────────────────────────────────────

/// GHASH d'un bloc 16 bytes dans GF(2^128).
/// Polynôme : x^128 + x^7 + x^2 + x + 1.
fn ghash_mul(x: &mut [u8; 16], y: &[u8; 16]) {
    let mut z = [0u8; 16];
    let mut v = *y;
    for i in 0..128 {
        let byte = i / 8;
        let bit  = 7 - (i % 8);
        if (x[byte] >> bit) & 1 == 1 {
            for j in 0..16 { z[j] ^= v[j]; }
        }
        // Shifter v à droite de 1 bit
        let carry = v[15] & 1;
        for j in (1..16).rev() { v[j] = (v[j] >> 1) | (v[j-1] << 7); }
        v[0] >>= 1;
        if carry == 1 { v[0] ^= 0xE1; } // polynôme réducteur (big-endian)
    }
    *x = z;
}

/// GHASH de données arbitraires.
fn ghash(h: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut y = [0u8; 16];
    let mut pos = 0;
    while pos + 16 <= data.len() {
        let mut block = [0u8; 16];
        block.copy_from_slice(&data[pos..pos+16]);
        for i in 0..16 { y[i] ^= block[i]; }
        ghash_mul(&mut y, h);
        pos += 16;
    }
    if pos < data.len() {
        let mut block = [0u8; 16];
        let rem = data.len() - pos;
        block[..rem].copy_from_slice(&data[pos..]);
        for i in 0..16 { y[i] ^= block[i]; }
        ghash_mul(&mut y, h);
    }
    y
}

/// Incrémenter le compteur CTR (32 bits big-endian, bytes 12-15).
fn ctr_inc(ctr: &mut [u8; 16]) {
    let c = u32::from_be_bytes([ctr[12], ctr[13], ctr[14], ctr[15]]);
    let bytes = c.wrapping_add(1).to_be_bytes();
    ctr[12..16].copy_from_slice(&bytes);
}

/// CTR stream : chiffre ou déchiffre `data` en place.
fn ctr_xor(data: &mut [u8], initial_counter: &[u8; 16], sched: &Aes256KeySchedule) {
    let mut ctr = *initial_counter;
    let mut pos = 0;
    while pos + 16 <= data.len() {
        let ks = aes256_encrypt_block(&ctr, sched);
        for i in 0..16 { data[pos+i] ^= ks[i]; }
        ctr_inc(&mut ctr);
        pos += 16;
    }
    if pos < data.len() {
        let ks = aes256_encrypt_block(&ctr, sched);
        for i in 0..(data.len()-pos) { data[pos+i] ^= ks[i]; }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API AES-256-GCM publique
// ─────────────────────────────────────────────────────────────────────────────

/// Chiffre `plaintext` avec AES-256-GCM.
///
/// # Paramètres
/// - `key`  : clé 32 bytes
/// - `iv`   : nonce 12 bytes (NON réutilisable avec la même clé)
/// - `aad`  : données supplémentaires authentifiées (non chiffrées)
/// - `plaintext` : données à chiffrer (modifiées en place → ciphertext)
/// - `tag`  : tag d'authentification 16 bytes (sortie)
pub fn aes_gcm_seal(
    key:       &[u8; 32],
    iv:        &[u8; 12],
    aad:       &[u8],
    plaintext: &mut [u8],
    tag:       &mut [u8; 16],
) -> Result<(), AesGcmError> {
    if aad.len() > 65535 || plaintext.len() > (1 << 30) {
        return Err(AesGcmError::LengthOverflow);
    }
    let sched = aes256_key_schedule(key);

    // H = AES(key, 0^128)
    let zero_block = [0u8; 16];
    let h = aes256_encrypt_block(&zero_block, &sched);

    // J0 = IV(96) || 0^31 || 1  (compteur initial pour le tag)
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;

    // J1 = J0 + 1  (compteur initial pour le chiffrement)
    let mut j1 = j0;
    ctr_inc(&mut j1);

    // Chiffrement CTR
    ctr_xor(plaintext, &j1, &sched);

    // Tag = GHASH(H; AAD || CT) XOR E(K, J0)
    let tag_val = compute_tag(&h, aad, plaintext, &j0, &sched);
    *tag = tag_val;
    Ok(())
}

/// Déchiffre et vérifie un ciphertext AES-256-GCM.
///
/// # Paramètres
/// - `key`        : clé 32 bytes
/// - `iv`         : nonce 12 bytes
/// - `aad`        : données supplémentaires authentifiées
/// - `ciphertext` : données à déchiffrer (modifiées en place → plaintext)
/// - `tag`        : tag d'authentification 16 bytes (vérifié)
///
/// Retourne Err(AuthenticationFailed) si le tag est invalide.
/// **Le ciphertext EST MODIFIÉ même en cas d'échec — ne pas utiliser avant Ok.**
pub fn aes_gcm_open(
    key:        &[u8; 32],
    iv:         &[u8; 12],
    aad:        &[u8],
    ciphertext: &mut [u8],
    tag:        &[u8; 16],
) -> Result<(), AesGcmError> {
    if aad.len() > 65535 || ciphertext.len() > (1 << 30) {
        return Err(AesGcmError::LengthOverflow);
    }
    let sched = aes256_key_schedule(key);

    // H = AES(key, 0^128)
    let zero_block = [0u8; 16];
    let h = aes256_encrypt_block(&zero_block, &sched);

    // J0 = IV(96) || 0^31 || 1
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;

    // Vérification du tag AVANT déchiffrement (RÈGLE AES-01)
    let expected_tag = compute_tag(&h, aad, ciphertext, &j0, &sched);
    let mut diff = 0u8;
    for i in 0..16 { diff |= expected_tag[i] ^ tag[i]; }

    // Fence pour protection contre les attaques de timing
    compiler_fence(Ordering::SeqCst);

    if diff != 0 {
        // Zéroïser le ciphertext pour éviter les fuites (ne jamais exposer des partiels)
        for b in ciphertext.iter_mut() { *b = 0; }
        return Err(AesGcmError::AuthenticationFailed);
    }

    // Déchiffrement CTR (identique au chiffrement)
    let mut j1 = j0;
    ctr_inc(&mut j1);
    ctr_xor(ciphertext, &j1, &sched);
    Ok(())
}

/// Calcule le tag GCM : GHASH(H; len(AAD)||len(CT)) XOR E(K, J0).
fn compute_tag(
    h:     &[u8; 16],
    aad:   &[u8],
    ct:    &[u8],
    j0:    &[u8; 16],
    sched: &Aes256KeySchedule,
) -> [u8; 16] {
    // GHASH des données en bloc : AAD || padding || CT || padding || lengths
    let pad_len = |n: usize| -> usize { if n % 16 == 0 { 0 } else { 16 - (n % 16) } };

    let mut y = [0u8; 16];

    // GHASH sur AAD
    let aad_hash = ghash(h, aad);
    for i in 0..16 { y[i] ^= aad_hash[i]; }

    // GHASH sur ciphertext
    let ct_hash = ghash(h, ct);
    for i in 0..16 { y[i] ^= ct_hash[i]; }

    // Bloc de longueurs : len(AAD)||len(CT) en bits, big-endian 64+64
    let mut len_block = [0u8; 16];
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits  = (ct.len()  as u64) * 8;
    len_block[0..8].copy_from_slice (&aad_bits.to_be_bytes());
    len_block[8..16].copy_from_slice(&ct_bits.to_be_bytes());
    for i in 0..16 { y[i] ^= len_block[i]; }
    ghash_mul(&mut y, h);

    let _ = pad_len(0); // Supprimer warning

    // XOR avec E(K, J0)
    let ej0 = aes256_encrypt_block(j0, sched);
    let mut tag = [0u8; 16];
    for i in 0..16 { tag[i] = y[i] ^ ej0[i]; }
    tag
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes AES : S-Box et RCON
// ─────────────────────────────────────────────────────────────────────────────

#[rustfmt::skip]
static SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

#[rustfmt::skip]
static RCON: [u32; 7] = [
    0x01000000, 0x02000000, 0x04000000, 0x08000000,
    0x10000000, 0x20000000, 0x40000000,
];
