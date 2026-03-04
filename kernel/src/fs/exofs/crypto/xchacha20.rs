//! XChaCha20-BLAKE3 AEAD — implémentation ExoFS, algorithme RFC 8439 + BLAKE3 MAC.
//!
//! # Construction AEAD : XChaCha20-BLAKE3 (Encrypt-then-MAC)
//!
//! Crates utilisées :
//!   - `blake3 v1`  (features = ["pure"]) — uniquement pour le MAC keyed hash
//!
//! ## Pourquoi pas les crates `chacha20` / `chacha20poly1305` ?
//!
//! - `chacha20poly1305 v0.10` : dépend de `poly1305 v0.8` → LLVM split SSE2
//! - `chacha20 v0.9`          : déclenche ÉGALEMENT le LLVM split 128-bit
//!   sur `x86_64-unknown-none` (types génériques de `cipher` crate).
//!
//! Seul le keystream ChaCha20 est implémenté localement (algorithme public RFC 8439,
//! arithmétique u32 pure, zéro u128/SIMD). Le MAC utilise `blake3` (externe validé).
//!
//! ## Construction XChaCha20-BLAKE3 (Encrypt-then-MAC)
//!
//! ```text
//! ikm       = key (32 B) ‖ nonce (24 B)
//! mac_key   = BLAKE3-derive-key("ExoFS-XChaCha20-BLAKE3-MAC-v1", ikm)
//! ct        = XChaCha20(key, nonce, plaintext)         [crate chacha20]
//! tag[0:16] = BLAKE3-keyed-hash(mac_key,               [crate blake3]
//!               LE64(len_aad) ‖ aad ‖ LE64(len_ct) ‖ ct)[:16]
//! ```
//!
//! Propriétés : confidentialité RFC 8439, authenticité 128 bits, IND-CCA2.
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.
//! S-06 / LAC-04  : nonces dérivés d'un compteur global + RDRAND.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::entropy::ENTROPY_POOL;

// ─────────────────────────────────────────────────────────────────────────────
// Compteur global de nonces (S-06 / LAC-04)
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur monotone global pour la génération de nonces XChaCha20.
///
/// Incrémenté à chaque appel à `next_nonce()` quel que soit le contexte.
/// Garantit que deux AeadContext distincts utilisant la même clé ne produisent
/// JAMAIS le même nonce (CRYPTO-NONCE / CAP-05 niveau crypto).
static GLOBAL_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);


// ─────────────────────────────────────────────────────────────────────────────
// Types publics
// ─────────────────────────────────────────────────────────────────────────────

/// Nonce XChaCha20 (24 octets = 192 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nonce(pub [u8; 24]);

/// Tag d'authentification Poly1305 (16 octets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag(pub [u8; 16]);

/// Clé symétrique 256 bits — zeroize-on-drop.
#[derive(Clone)]
pub struct XChaCha20Key(pub [u8; 32]);

impl core::fmt::Debug for XChaCha20Key {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "XChaCha20Key(<redacted>)")
    }
}

impl Drop for XChaCha20Key {
    fn drop(&mut self) {
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

impl Nonce {
    /// Crée un nonce à partir de 24 octets.
    pub const fn from_bytes(b: [u8; 24]) -> Self { Self(b) }

    /// Nonce nul (utilisation tests uniquement).
    pub const fn zero() -> Self { Self([0u8; 24]) }

    /// Incrémente le nonce en little-endian (ARITH-02).
    pub fn increment(&mut self) {
        for byte in self.0.iter_mut() {
            let (v, overflow) = byte.overflowing_add(1);
            *byte = v;
            if !overflow { break; }
        }
    }
}

impl Tag {
    /// Comparaison en temps constant.
    pub fn constant_time_eq(&self, other: &Tag) -> bool {
        constant_time_eq_16(&self.0, &other.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Contexte AEAD
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte AEAD réutilisable (clé + compteur de nonce).
pub struct AeadContext {
    key:     XChaCha20Key,
    counter: u64,
}

impl AeadContext {
    /// Crée un contexte depuis une clé.
    pub fn new(key: XChaCha20Key) -> Self {
        Self { key, counter: 0 }
    }

    /// Chiffre et retourne (ciphertext, tag). Le nonce est généré automatiquement.
    pub fn seal(&mut self, aad: &[u8], plaintext: &[u8]) -> ExofsResult<(Vec<u8>, Nonce, Tag)> {
        let nonce = self.next_nonce();
        let (ct, tag) = XChaCha20Poly1305::encrypt(&self.key, &nonce, aad, plaintext)?;
        Ok((ct, nonce, tag))
    }

    /// Déchiffre et vérifie le tag.
    pub fn open(
        &self,
        aad: &[u8],
        ciphertext: &[u8],
        nonce: &Nonce,
        tag: &Tag,
    ) -> ExofsResult<Vec<u8>> {
        XChaCha20Poly1305::decrypt(&self.key, nonce, aad, ciphertext, tag)
    }

    fn next_nonce(&mut self) -> Nonce {
        // S-06 / LAC-04 : compteur GLOBAL monotone + RDRAND comme diversifiant.
        // Le compteur global garantit l'unicité même si deux AeadContext
        // partagent la même clé ou si RDRAND est faible.
        let counter  = GLOBAL_NONCE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let entropy  = ENTROPY_POOL.random_u64(); // ChaCha20-DRNG + TSC
        let mut n    = [0u8; 24];
        n[0..8].copy_from_slice(&counter.to_le_bytes());
        n[8..16].copy_from_slice(&entropy.to_le_bytes());
        // Bytes 16-23 = XOR des deux pour mélanger sans répétition
        let mixed = counter.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(entropy);
        n[16..24].copy_from_slice(&mixed.to_le_bytes());
        Nonce(n)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XChaCha20-Poly1305 AEAD
// ─────────────────────────────────────────────────────────────────────────────

/// AEAD XChaCha20-Poly1305 (RFC 8439 + XChaCha extension).
pub struct XChaCha20Poly1305;

impl XChaCha20Poly1305 {
    /// Chiffre `plaintext` → (ciphertext, tag).
    ///
    /// Construction Encrypt-then-MAC :
    ///   ct  = XChaCha20(key, nonce, plaintext)          [crate chacha20]
    ///   tag = BLAKE3-keyed-hash(mac_key, aad ‖ ct)[:16] [crate blake3]
    ///
    /// OOM-02 : try_reserve avant tout push.
    pub fn encrypt(
        key:       &XChaCha20Key,
        nonce:     &Nonce,
        aad:       &[u8],
        plaintext: &[u8],
    ) -> ExofsResult<(Vec<u8>, Tag)> {
        let mut ct = Vec::new();
        ct.try_reserve(plaintext.len()).map_err(|_| ExofsError::NoMemory)?;
        ct.extend_from_slice(plaintext);
        xchacha20_apply(&key.0, &nonce.0, &mut ct);
        let tag = compute_tag(&key.0, &nonce.0, aad, &ct);
        Ok((ct, tag))
    }

    /// Déchiffre `ciphertext`, vérifie le `tag` (Encrypt-then-MAC).
    ///
    /// Le tag est vérifié AVANT le déchiffrement (IND-CCA2).
    pub fn decrypt(
        key:        &XChaCha20Key,
        nonce:      &Nonce,
        aad:        &[u8],
        ciphertext: &[u8],
        tag:        &Tag,
    ) -> ExofsResult<Vec<u8>> {
        // 1. Vérification du tag en temps constant (MAC-before-decrypt).
        let expected = compute_tag(&key.0, &nonce.0, aad, ciphertext);
        if !expected.constant_time_eq(tag) {
            return Err(ExofsError::CorruptedStructure);
        }
        // 2. Déchiffrement uniquement si le tag est valide.
        let mut pt = Vec::new();
        pt.try_reserve(ciphertext.len()).map_err(|_| ExofsError::NoMemory)?;
        pt.extend_from_slice(ciphertext);
        xchacha20_apply(&key.0, &nonce.0, &mut pt);
        Ok(pt)
    }

    /// Taille du ciphertext = taille du plaintext (stream cipher, pas de padding).
    pub const fn ciphertext_len(plaintext_len: usize) -> usize {
        plaintext_len
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Primitives internes — ChaCha20 RFC 8439 (arithmétique u32 pure, pas de SIMD)
// ─────────────────────────────────────────────────────────────────────────────

/// Quarter-round ChaCha20 (RFC 8439 §2.1).
/// Pure u32 — aucun u128, aucun SIMD, compatible x86_64-unknown-none.
#[inline(always)]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left( 8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left( 7);
}

/// Bloc ChaCha20 (RFC 8439 §2.3).
#[inline]
fn chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut s: [u32; 16] = [
        0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574,
        u32::from_le_bytes(key[ 0.. 4].try_into().unwrap()),
        u32::from_le_bytes(key[ 4.. 8].try_into().unwrap()),
        u32::from_le_bytes(key[ 8..12].try_into().unwrap()),
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
        quarter_round(&mut s, 0, 4,  8, 12); quarter_round(&mut s, 1, 5,  9, 13);
        quarter_round(&mut s, 2, 6, 10, 14); quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15); quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7,  8, 13); quarter_round(&mut s, 3, 4,  9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// HChaCha20 — dérive une sous-clé 32 B à partir d'une clé et des 16 premiers octets
/// d'un nonce XChaCha20 (RFC draft-irtf-cfrg-xchacha §2.2).
#[inline]
fn hchacha20(key: &[u8; 32], nonce16: &[u8; 16]) -> [u8; 32] {
    let mut s: [u32; 16] = [
        0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574,
        u32::from_le_bytes(key[ 0.. 4].try_into().unwrap()),
        u32::from_le_bytes(key[ 4.. 8].try_into().unwrap()),
        u32::from_le_bytes(key[ 8..12].try_into().unwrap()),
        u32::from_le_bytes(key[12..16].try_into().unwrap()),
        u32::from_le_bytes(key[16..20].try_into().unwrap()),
        u32::from_le_bytes(key[20..24].try_into().unwrap()),
        u32::from_le_bytes(key[24..28].try_into().unwrap()),
        u32::from_le_bytes(key[28..32].try_into().unwrap()),
        u32::from_le_bytes(nonce16[ 0.. 4].try_into().unwrap()),
        u32::from_le_bytes(nonce16[ 4.. 8].try_into().unwrap()),
        u32::from_le_bytes(nonce16[ 8..12].try_into().unwrap()),
        u32::from_le_bytes(nonce16[12..16].try_into().unwrap()),
    ];
    for _ in 0..10 {
        quarter_round(&mut s, 0, 4,  8, 12); quarter_round(&mut s, 1, 5,  9, 13);
        quarter_round(&mut s, 2, 6, 10, 14); quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15); quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7,  8, 13); quarter_round(&mut s, 3, 4,  9, 14);
    }
    let mut out = [0u8; 32];
    out[ 0.. 4].copy_from_slice(&s[ 0].to_le_bytes()); out[ 4.. 8].copy_from_slice(&s[ 1].to_le_bytes());
    out[ 8..12].copy_from_slice(&s[ 2].to_le_bytes()); out[12..16].copy_from_slice(&s[ 3].to_le_bytes());
    out[16..20].copy_from_slice(&s[12].to_le_bytes()); out[20..24].copy_from_slice(&s[13].to_le_bytes());
    out[24..28].copy_from_slice(&s[14].to_le_bytes()); out[28..32].copy_from_slice(&s[15].to_le_bytes());
    out
}

/// Applique XChaCha20 in-place sur `buf` (chiffrement = déchiffrement).
///
/// Construction : subkey = HChaCha20(key, nonce[0..16]),
///               keystream = ChaCha20(subkey, nonce[16..24] padded, counter=1).
#[inline]
fn xchacha20_apply(key: &[u8; 32], nonce: &[u8; 24], buf: &mut [u8]) {
    let subkey: [u8; 32] = hchacha20(key, nonce[..16].try_into().unwrap());
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce[4..12].copy_from_slice(&nonce[16..24]);
    let mut counter: u32 = 1;
    let mut i = 0;
    while i < buf.len() {
        let block = chacha20_block(&subkey, &chacha_nonce, counter);
        let n = (buf.len() - i).min(64);
        for j in 0..n { buf[i + j] ^= block[j]; }
        i += n;
        counter = counter.wrapping_add(1);
    }
}

/// Calcule le tag BLAKE3-128 couvrant `(aad, ciphertext)` pour ce `(key, nonce)`.
///
/// Dérivation nonce-dépendante de la clé MAC via BLAKE3 KDF :
/// ```text
/// ikm     = key (32 B) ‖ nonce (24 B)
/// mac_key = BLAKE3-derive-key("ExoFS-XChaCha20-BLAKE3-MAC-v1", ikm)
/// tag     = BLAKE3-keyed-hash(mac_key, LE64(len_aad)‖aad‖LE64(len_ct)‖ct)[:16]
/// ```
fn compute_tag(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], ct: &[u8]) -> Tag {
    let mut ikm = [0u8; 56];
    ikm[..32].copy_from_slice(key);
    ikm[32..56].copy_from_slice(nonce);
    let mac_key: [u8; 32] = blake3::derive_key("ExoFS-XChaCha20-BLAKE3-MAC-v1", &ikm);
    let mut h = blake3::Hasher::new_keyed(&mac_key);
    h.update(&(aad.len() as u64).to_le_bytes());
    h.update(aad);
    h.update(&(ct.len() as u64).to_le_bytes());
    h.update(ct);
    let out = h.finalize();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&out.as_bytes()[..16]);
    Tag(tag)
}

/// Comparaison en temps constant (résistance au timing attack).
#[inline]
fn constant_time_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff = 0u8;
    let mut i = 0usize;
    while i < 16 {
        diff |= a[i] ^ b[i];
        i = i.wrapping_add(1);
    }
    diff == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key()   -> XChaCha20Key { XChaCha20Key([0x42u8; 32]) }
    fn test_nonce() -> Nonce        { Nonce([0x11u8; 24]) }

    #[test] fn test_encrypt_decrypt_roundtrip() {
        let key   = test_key();
        let nonce = test_nonce();
        let msg   = b"ExoFS XChaCha20-BLAKE3 roundtrip test";
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"aad", msg).unwrap();
        assert_ne!(ct.as_slice(), msg.as_ref(), "ciphertext != plaintext");
        let pt = XChaCha20Poly1305::decrypt(&key, &nonce, b"aad", &ct, &tag).unwrap();
        assert_eq!(pt.as_slice(), msg.as_ref());
    }

    #[test] fn test_encrypt_decrypt_empty() {
        let key   = test_key();
        let nonce = test_nonce();
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"", &[]).unwrap();
        let pt = XChaCha20Poly1305::decrypt(&key, &nonce, b"", &ct, &tag).unwrap();
        assert!(pt.is_empty());
    }

    #[test] fn test_wrong_tag_rejected() {
        let key   = test_key();
        let nonce = test_nonce();
        let (ct, _) = XChaCha20Poly1305::encrypt(&key, &nonce, b"aad", b"secret").unwrap();
        assert!(XChaCha20Poly1305::decrypt(&key, &nonce, b"aad", &ct, &Tag([0u8;16])).is_err());
    }

    #[test] fn test_tampered_ciphertext_rejected() {
        let key   = test_key();
        let nonce = test_nonce();
        let (mut ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"", b"data").unwrap();
        ct[0] ^= 0xFF;
        assert!(XChaCha20Poly1305::decrypt(&key, &nonce, b"", &ct, &tag).is_err());
    }

    #[test] fn test_wrong_aad_rejected() {
        let key   = test_key();
        let nonce = test_nonce();
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"aad1", b"msg").unwrap();
        assert!(XChaCha20Poly1305::decrypt(&key, &nonce, b"aad2", &ct, &tag).is_err());
    }

    #[test] fn test_wrong_nonce_rejected() {
        let key    = test_key();
        let nonce1 = Nonce([0x01u8; 24]);
        let nonce2 = Nonce([0x02u8; 24]);
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce1, &[], b"data").unwrap();
        assert!(XChaCha20Poly1305::decrypt(&key, &nonce2, &[], &ct, &tag).is_err());
    }

    #[test] fn test_ciphertext_len() {
        assert_eq!(XChaCha20Poly1305::ciphertext_len(0),   0);
        assert_eq!(XChaCha20Poly1305::ciphertext_len(100), 100);
    }

    #[test] fn test_nonce_increment_overflow() {
        let mut n = Nonce([0xFFu8; 24]);
        n.increment();
        assert_eq!(n.0, [0u8; 24]);
    }

    #[test] fn test_nonce_increment_basic() {
        let mut n = Nonce::zero();
        n.increment();
        assert_eq!(n.0[0], 1);
    }

    #[test] fn test_constant_time_eq_same() {
        assert!(constant_time_eq_16(&[0xABu8; 16], &[0xABu8; 16]));
    }

    #[test] fn test_constant_time_eq_diff() {
        let a = [0u8; 16];
        let mut b = [0u8; 16]; b[7] = 1;
        assert!(!constant_time_eq_16(&a, &b));
    }

    #[test] fn test_aead_context_seal_open() {
        let key     = test_key();
        let mut ctx = AeadContext::new(key.clone());
        let (ct, nonce, tag) = ctx.seal(b"aad", b"hello kernel").unwrap();
        let ctx2 = AeadContext::new(key);
        let pt   = ctx2.open(b"aad", &ct, &nonce, &tag).unwrap();
        assert_eq!(pt.as_slice(), b"hello kernel");
    }

    #[test] fn test_aead_context_unique_nonces() {
        let key     = test_key();
        let mut ctx = AeadContext::new(key);
        let (_, n1, _) = ctx.seal(b"", b"a").unwrap();
        let (_, n2, _) = ctx.seal(b"", b"b").unwrap();
        assert_ne!(n1.0, n2.0, "nonces doivent être uniques");
    }

    #[test] fn test_large_plaintext() {
        let key   = test_key();
        let nonce = test_nonce();
        let msg: alloc::vec::Vec<u8> = (0..4096u16).map(|i| (i % 256) as u8).collect();
        let (ct, tag) = XChaCha20Poly1305::encrypt(&key, &nonce, b"large", &msg).unwrap();
        let pt = XChaCha20Poly1305::decrypt(&key, &nonce, b"large", &ct, &tag).unwrap();
        assert_eq!(pt, msg);
    }
}
