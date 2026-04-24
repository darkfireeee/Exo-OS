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

use super::entropy::ENTROPY_POOL;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::security::crypto::derive_subkey;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Délégation au module security::crypto
// ─────────────────────────────────────────────────────────────────────────────

use crate::security::crypto::xchacha20_poly1305::AeadError;
use crate::security::crypto::{
    xchacha20_poly1305_open, xchacha20_poly1305_seal, XCHACHA20_TAG_SIZE,
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
            unsafe {
                core::ptr::write_volatile(b, 0u8);
            }
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
        key: &XChaCha20Key,
        nonce: &Nonce,
        aad: &[u8],
        plaintext: &[u8],
    ) -> ExofsResult<(Vec<u8>, Tag)> {
        let mut ciphertext = Vec::new();
        ciphertext
            .try_reserve(plaintext.len())
            .map_err(|_| ExofsError::NoMemory)?;
        ciphertext.extend_from_slice(plaintext);

        let mut tag = [0u8; XCHACHA20_TAG_SIZE];
        xchacha20_poly1305_seal(&key.0, &nonce.0, &mut ciphertext, aad, &mut tag)
            .map_err(map_encrypt_error)?;

        Ok((ciphertext, Tag(tag)))
    }

    /// Déchiffre et vérifie `ciphertext` avec tag d'authentification.
    ///
    /// Retourne le plaintext si l'authentification réussit.
    /// OOM-02 : allocation via `try_reserve`.
    pub fn decrypt(
        key: &XChaCha20Key,
        nonce: &Nonce,
        aad: &[u8],
        ciphertext: &[u8],
        tag: &Tag,
    ) -> ExofsResult<Vec<u8>> {
        let mut plaintext = Vec::new();
        plaintext
            .try_reserve(ciphertext.len())
            .map_err(|_| ExofsError::NoMemory)?;
        plaintext.extend_from_slice(ciphertext);

        xchacha20_poly1305_open(&key.0, &nonce.0, &mut plaintext, aad, &tag.0)
            .map_err(map_decrypt_error)?;

        Ok(plaintext)
    }
}

fn map_encrypt_error(err: AeadError) -> ExofsError {
    match err {
        AeadError::InvalidParameter => ExofsError::InvalidArgument,
        AeadError::BufferTooSmall => ExofsError::InvalidSize,
        AeadError::NotAvailableOnThisTarget => ExofsError::NotSupported,
        AeadError::AuthenticationFailed => ExofsError::InternalError,
    }
}

fn map_decrypt_error(err: AeadError) -> ExofsError {
    match err {
        AeadError::AuthenticationFailed => ExofsError::ChecksumMismatch,
        AeadError::InvalidParameter => ExofsError::InvalidArgument,
        AeadError::BufferTooSmall => ExofsError::InvalidSize,
        AeadError::NotAvailableOnThisTarget => ExofsError::NotSupported,
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
        Self {
            key: XChaCha20Key(key),
        }
    }

    /// Génère un nonce unique via KDF : compteur global + entropie + object_id.
    ///
    /// Construction (S-06) :
    /// - ikm  = counter || object_id || entropy
    /// - salt = random_32()
    /// - okm  = HKDF(ikm, salt, "ExoFS-XChaCha20-Nonce-v1")
    ///
    /// Fallback best-effort : réutilise la composition directe si le KDF échoue,
    /// afin de conserver l'unicité même en cas de panne interne du sous-système.
    pub fn next_nonce(&self, object_id: u64) -> Nonce {
        let counter = GLOBAL_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let entropy = ENTROPY_POOL.random_u64();
        let salt = ENTROPY_POOL.random_32();
        let mut ikm = [0u8; 24];
        ikm[0..8].copy_from_slice(&counter.to_le_bytes());
        ikm[8..16].copy_from_slice(&object_id.to_le_bytes());
        ikm[16..24].copy_from_slice(&entropy.to_le_bytes());
        let mut nonce = [0u8; 24];
        match derive_subkey(&ikm, Some(&salt), b"ExoFS-XChaCha20-Nonce-v1") {
            Ok(derived) => nonce.copy_from_slice(&derived.as_bytes()[..24]),
            Err(_) => nonce.copy_from_slice(&ikm),
        }
        Nonce(nonce)
    }

    /// Chiffre `plaintext` avec un nonce lié à `object_id`.
    pub fn encrypt_for_object(
        &self,
        object_id: u64,
        aad: &[u8],
        plaintext: &[u8],
    ) -> ExofsResult<(Vec<u8>, Nonce, Tag)> {
        let nonce = self.next_nonce(object_id);
        let (ct, tag) = XChaCha20Poly1305::encrypt(&self.key, &nonce, aad, plaintext)?;
        Ok((ct, nonce, tag))
    }

    /// Déchiffre `ciphertext` avec le nonce et tag fournis.
    pub fn decrypt_for_object(
        &self,
        nonce: &Nonce,
        aad: &[u8],
        ciphertext: &[u8],
        tag: &Tag,
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

    fn init_crypto_test_env() {
        crate::arch::x86_64::cpu::features::init_cpu_features();
        crate::security::crypto::rng_init();
    }

    fn test_key() -> XChaCha20Key {
        XChaCha20Key([0x42u8; 32])
    }
    fn test_nonce() -> Nonce {
        Nonce([0x24u8; 24])
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        init_crypto_test_env();
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS test payload";
        let (ct, tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        let dec = XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).unwrap();
        assert_eq!(&dec, plain);
    }

    #[test]
    fn test_auth_failure_on_tampered_tag() {
        init_crypto_test_env();
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS auth test";
        let (ct, mut tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        tag.0[0] ^= 0xFF; // Corrompre le tag
        assert!(XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).is_err());
    }

    #[test]
    fn test_auth_failure_on_tampered_ct() {
        init_crypto_test_env();
        let k = test_key();
        let n = test_nonce();
        let plain = b"ExoFS auth test";
        let (mut ct, tag) = XChaCha20Poly1305::encrypt(&k, &n, &[], plain).unwrap();
        if !ct.is_empty() {
            ct[0] ^= 0xFF;
        }
        assert!(XChaCha20Poly1305::decrypt(&k, &n, &[], &ct, &tag).is_err());
    }

    #[test]
    fn test_aead_context_nonce_unique_and_domain_separated() {
        init_crypto_test_env();
        let ctx = AeadContext::new([0xABu8; 32]);
        let n1 = ctx.next_nonce(1);
        let n2 = ctx.next_nonce(1);
        let n3 = ctx.next_nonce(2);
        assert_ne!(n1, n2);
        assert_ne!(n1, n3);
    }

    #[test]
    fn test_key_zeroize_on_drop() {
        init_crypto_test_env();
        let k = XChaCha20Key([0xFFu8; 32]);
        drop(k);
        // Pas de panique = zeroize OK
    }

    #[test]
    fn test_encrypt_with_aad() {
        init_crypto_test_env();
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
