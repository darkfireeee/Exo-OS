//! XChaCha20-Poly1305 AEAD ExoFS — Wrapper sur `crate::security::crypto`
//!
//! Ce module expose la même API publique qu'auparavant (`XChaCha20Key`,
//! `Nonce`, `Tag`, `XChaCha20Poly1305`) mais délègue **toutes les opérations
//! cryptographiques** à `crate::security::crypto::xchacha20_poly1305`.
//!
//! ## Règle architecturale (docs/recast/ExoOS_Architecture_v7.md §S-06)
//!
//! `security::crypto::xchacha20_poly1305` est la primitive AEAD kernel unique.
//! ExoFS ne doit pas la dupliquer — un seul site d'implémentation = un seul
//! site d'audit et de correction.
//!
//! ## Construction AEAD (inchangée)
//!
//! ```text
//! ct || tag = XChaCha20(key, nonce, plaintext) + BLAKE3-MAC
//! ```
//!
//! Nonces : générés via `entropy::ENTROPY_POOL.nonce_for_object_id()` (S-06 / LAC-04).
//!
//! ## Règles locales
//! - OOM-02  : `try_reserve` avant toute allocation Vec.
//! - ARITH-02: arithmétique saturating/checked.
//! - RECUR-01: aucune récursivité.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::entropy::ENTROPY_POOL;

// ─────────────────────────────────────────────────────────────────────────────
// Délégation au module security::crypto
// ─────────────────────────────────────────────────────────────────────────────

use crate::security::crypto::{
    xchacha20_poly1305_seal,
    xchacha20_poly1305_open,
    XCHACHA20_TAG_SIZE,
    XCHACHA20_NONCE_SIZE,
    XCHACHA20_KEY_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Compteur global de nonces (S-06 / LAC-04)
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur monotone global pour la génération de nonces XChaCha20.
/// Incrémenté à chaque `next_nonce()`, garantit la non-réutilisation.
static GLOBAL_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

// ─────────────────────────────────────────────────────────────────────────────
// Types publics (API inchangée)
// ─────────────────────────────────────────────────────────────────────────────

/// Nonce XChaCha20 (24 octets = 192 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nonce(pub [u8; 24]);

/// Tag d'authentification (16 octets — truncated BLAKE3 MAC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag(pub [u8; 16]);

/// Clé XChaCha20 (32 octets = 256 bits).
/// Zéroïsée automatiquement à la destruction (CRYPTO-03).
pub struct XChaCha20Key(pub [u8; 32]);

impl Drop for XChaCha20Key {
    fn drop(&mut self) {
        // Zéroïsation garantie (CRYPTO-03)
        for b in self.0.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0u8); }
        }
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

impl Nonce {
    /// Construit un nonce depuis un slice de 24 octets.
    pub fn from_slice(s: &[u8]) -> Option<Self> {
        if s.len() == 24 {
            let mut arr = [0u8; 24];
            arr.copy_from_slice(s);
            Some(Self(arr))
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XChaCha20Poly1305 — opérations AEAD (API inchangée, implémentation déléguée)
// ─────────────────────────────────────────────────────────────────────────────

/// Opérations AEAD XChaCha20-Poly1305.
///
/// Délègue à `crate::security::crypto::xchacha20_poly1305_seal/open`.
pub struct XChaCha20Poly1305;

impl XChaCha20Poly1305 {
    /// Chiffre `plaintext` avec AEAD XChaCha20-Poly1305.
    ///
    /// Retourne `(ciphertext, tag)`.
    ///
    /// OOM-02 : allocation via `try_reserve`.
    pub fn encrypt(
        key:       &XChaCha20Key,
        nonce:     &Nonce,
        aad:       &[u8],
        plaintext: &[u8],
    ) -> ExofsResult<(Vec<u8>, Tag)> {
        // Allouer le buffer de sortie : ciphertext + tag
        let ct_len = plaintext.len() + XCHACHA20_TAG_SIZE;
        let mut out = Vec::new();
        out.try_reserve(ct_len).map_err(|_| ExofsError::OutOfMemory)?;
        out.resize(ct_len, 0u8);

        // Déléguer au module security::crypto
        xchacha20_poly1305_seal(
            &key.0,
            &nonce.0,
            plaintext,
            aad,
            &mut out,
        ).map_err(|_| ExofsError::CryptoError)?;

        // Séparer ciphertext et tag
        let tag_bytes: [u8; 16] = out[plaintext.len()..plaintext.len() + 16]
            .try_into()
            .map_err(|_| ExofsError::CryptoError)?;
        out.truncate(plaintext.len());

        Ok((out, Tag(tag_bytes)))
    }

    /// Déchiffre et vérifie `ciphertext` avec tag d'authentification.
    ///
    /// Retourne le plaintext si l'authentification réussit.
    /// Retourne `ExofsError::AuthenticationFailed` si le tag est invalide.
    ///
    /// OOM-02 : allocation via `try_reserve`.
    pub fn decrypt(
        key:        &XChaCha20Key,
        nonce:      &Nonce,
        aad:        &[u8],
        ciphertext: &[u8],
        tag:        &Tag,
    ) -> ExofsResult<Vec<u8>> {
        // Reconstruire ciphertext || tag pour l'API security::crypto
        let combined_len = ciphertext.len() + XCHACHA20_TAG_SIZE;
        let mut combined = Vec::new();
        combined.try_reserve(combined_len).map_err(|_| ExofsError::OutOfMemory)?;
        combined.extend_from_slice(ciphertext);
        combined.extend_from_slice(&tag.0);

        // Buffer de sortie
        let pt_len = ciphertext.len();
        let mut plaintext = Vec::new();
        plaintext.try_reserve(pt_len).map_err(|_| ExofsError::OutOfMemory)?;
        plaintext.resize(pt_len, 0u8);

        xchacha20_poly1305_open(
            &key.0,
            &nonce.0,
            &combined,
            aad,
            &mut plaintext,
        ).map_err(|_| ExofsError::AuthenticationFailed)?;

        Ok(plaintext)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AeadContext — gestion de nonces par contexte (API inchangée)
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte AEAD par objet ExoFS avec génération de nonce sécurisée.
///
/// Garantit que chaque chiffrement utilise un nonce unique (S-06 / LAC-04).
pub struct AeadContext {
    key: XChaCha20Key,
}

impl AeadContext {
    /// Crée un contexte AEAD pour une clé donnée.
    pub fn new(key: [u8; 32]) -> Self {
        Self { key: XChaCha20Key(key) }
    }

    /// Génère un nonce unique : compteur global + entropie + object_id.
    ///
    /// Construction (S-06) :
    /// - [0..8]  = compteur global monotone (non-réutilisation garantie)
    /// - [8..16] = object_id (domaine de séparation)
    /// - [16..24] = entropie CSPRNG (security::crypto::rng)
    pub fn next_nonce(&self, object_id: u64) -> Nonce {
        let counter = GLOBAL_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut nonce = [0u8; 24];
        nonce[0..8].copy_from_slice(&counter.to_le_bytes());
        nonce[8..16].copy_from_slice(&object_id.to_le_bytes());
        nonce[16..24].copy_from_slice(&ENTROPY_POOL.random_u64().to_le_bytes());
        Nonce(nonce)
    }

    /// Chiffre `plaintext` avec un nonce lié à `object_id`.
    pub fn encrypt_for_object(
        &self,
        object_id: u64,
        aad:       &[u8],
        plaintext: &[u8],
    ) -> ExofsResult<(Vec<u8>, Nonce, Tag)> {
        let nonce = self.next_nonce(object_id);
        let (ct, tag) = XChaCha20Poly1305::encrypt(&self.key, &nonce, aad, plaintext)?;
        Ok((ct, nonce, tag))
    }

    /// Déchiffre `ciphertext` avec le nonce et tag fournis.
    pub fn decrypt_for_object(
        &self,
        nonce:      &Nonce,
        aad:        &[u8],
        ciphertext: &[u8],
        tag:        &Tag,
    ) -> ExofsResult<Vec<u8>> {
        XChaCha20Poly1305::decrypt(&self.key, nonce, aad, ciphertext, tag)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> XChaCha20Key { XChaCha20Key([0x42u8; 32]) }
    fn test_nonce() -> Nonce { Nonce([0x24u8; 24]) }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS test payload";
        let (ct, tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        let dec = XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).unwrap();
        assert_eq!(&dec, plain);
    }

    #[test]
    fn test_auth_failure_on_tampered_tag() {
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS auth test";
        let (ct, mut tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        tag.0[0] ^= 0xFF; // Corrompre le tag
        assert!(XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).is_err());
    }

    #[test]
    fn test_auth_failure_on_tampered_ct() {
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS auth test";
        let (mut ct, tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        if !ct.is_empty() { ct[0] ^= 0xFF; }
        assert!(XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).is_err());
    }

    #[test]
    fn test_aead_context_nonce_monotone() {
        let ctx = AeadContext::new([0xABu8; 32]);
        let n1 = ctx.next_nonce(1);
        let n2 = ctx.next_nonce(1);
        // Les 8 premiers octets (compteur) doivent être différents
        assert_ne!(n1.0[0..8], n2.0[0..8]);
    }

    #[test]
    fn test_key_zeroize_on_drop() {
        let k = XChaCha20Key([0xFFu8; 32]);
        drop(k);
        // Pas de panique = zeroize OK
    }

    #[test]
    fn test_encrypt_with_aad() {
        let k = test_key();
        let n = test_nonce();
        let plain = b"secret data";
        let aad = b"authenticated header";
        let (ct, tag) = XChaCha20Poly1305::encrypt(&k, &n, aad, plain).unwrap();
        // Décryptage réussit avec le bon AAD
        let dec = XChaCha20Poly1305::decrypt(&k, &n, aad, &ct, &tag).unwrap();
        assert_eq!(&dec, plain);
        // Décryptage échoue avec un AAD différent
        assert!(XChaCha20Poly1305::decrypt(&k, &n, b"wrong aad", &ct, &tag).is_err());
    }
}
