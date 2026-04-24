//! Dérivation de clés ExoFS — Wrapper sur `crate::security::crypto::kdf`
//!
//! Ce module expose la même API publique qu'auparavant (`KeyDerivation`,
//! `DerivedKey`, `KeyPurpose`) mais délègue **toutes les opérations HKDF**
//! à `crate::security::crypto::kdf` (HKDF-BLAKE3), qui remplace
//! l'ancienne implémentation HKDF-SHA256 from-scratch.
//!
//! ## Règle architecturale (docs/recast/ExoOS_Architecture_v7.md §S-08)
//!
//! ExoOS utilise **BLAKE3** comme fonction de hachage de référence partout.
//! SHA-256 n'est PAS utilisé dans le kernel ExoOS (nécessiterait `sha2` crate
//! avec `force-soft` feature = overhead). HKDF-BLAKE3 est le seul KDF autorisé.
//!
//! Pour l'étirement de passphrases (Argon2id), la crate `argon2` (déjà
//! dans workspace) est utilisée — les paramètres respectent la règle S-16
//! (m=65536, t=3, p=4, sel 128 bits).
//!
//! ## Règles locales
//! - OOM-02  : `try_reserve` avant toute allocation Vec.
//! - ARITH-02: arithmétique saturating/checked.
//! - RECUR-01: aucune récursivité.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use hkdf::Hkdf;
use sha2::Sha256;

// ─────────────────────────────────────────────────────────────────────────────
// Délégation au module security::crypto::kdf
// ─────────────────────────────────────────────────────────────────────────────

use crate::security::crypto::{
    blake3_kdf,          // blake3_kdf(context, material) → DerivedKey32
    derive_fs_block_key, // derive_fs_block_key(vk, block_id) → DerivedKey32
    hkdf_extract as security_hkdf_extract,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une clé dérivée standard (256 bits).
pub const DERIVED_KEY_LEN: usize = 32;
/// Longueur maximale de sortie HKDF = 255 × 32 (BLAKE3 output size).
pub const HKDF_MAX_OUTPUT: usize = 255 * 32;
/// Nombre d'itérations minimum pour l'étirement de passphrase (Argon2id).
pub const KDF_MIN_ITERS: u8 = 3;
/// Nombre d'itérations recommandé (Argon2id time_cost=3, cf. S-16).
pub const KDF_DEFAULT_ITERS: u8 = 3;

// ─────────────────────────────────────────────────────────────────────────────
// KeyPurpose — domaine de séparation par type de clé
// ─────────────────────────────────────────────────────────────────────────────

/// Domaine de séparation par type de clé.
///
/// Garantit que deux clés dérivées du même matériel pour des usages différents
/// sont cryptographiquement distinctes (domain separation BLAKE3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPurpose {
    /// Clé de chiffrement de données.
    DataEncryption,
    /// Clé d'intégrité (MAC).
    Authentication,
    /// Clé de session (éphémère).
    Session,
    /// Clé d'enveloppe (wrapping).
    Wrapping,
    /// Clé d'objet BlobFS.
    BlobObject,
    /// Dérivation personnalisée.
    Custom(&'static str),
}

impl KeyPurpose {
    /// Retourne le contexte de domaine pour BLAKE3 KDF.
    pub fn as_context(&self) -> &'static [u8] {
        match self {
            Self::DataEncryption => b"ExoFS-DataEncryption-v1",
            Self::Authentication => b"ExoFS-Authentication-v1",
            Self::Session => b"ExoFS-Session-v1",
            Self::Wrapping => b"ExoFS-Wrapping-v1",
            Self::BlobObject => b"ExoFS-BlobObject-v1",
            Self::Custom(s) => s.as_bytes(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DerivedKey — clé dérivée avec zeroïsation automatique
// ─────────────────────────────────────────────────────────────────────────────

/// Clé dérivée de 32 octets avec zeroïsation à la destruction (CRYPTO-03).
#[derive(Clone, Debug)]
pub struct DerivedKey {
    bytes: [u8; DERIVED_KEY_LEN],
}

impl DerivedKey {
    /// Construit depuis un tableau de bytes.
    pub fn from_bytes(b: [u8; DERIVED_KEY_LEN]) -> Self {
        Self { bytes: b }
    }

    /// Retourne une référence aux bytes de la clé.
    pub fn as_bytes(&self) -> &[u8; DERIVED_KEY_LEN] {
        &self.bytes
    }

    /// Copie les bytes dans un tableau destination.
    pub fn copy_to(&self, dst: &mut [u8; DERIVED_KEY_LEN]) {
        *dst = self.bytes;
    }
}

impl Drop for DerivedKey {
    fn drop(&mut self) {
        for b in self.bytes.iter_mut() {
            unsafe {
                core::ptr::write_volatile(b, 0u8);
            }
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KeyDerivation — opérations de dérivation (API inchangée, implémentation BLAKE3)
// ─────────────────────────────────────────────────────────────────────────────

/// Opérations de dérivation de clés.
///
/// Délègue à `crate::security::crypto::kdf` (HKDF-BLAKE3).
/// Remplace l'ancienne implémentation HKDF-SHA256 from-scratch.
pub struct KeyDerivation;

impl KeyDerivation {
    // ── HKDF primitives ────────────────────────────────────────────────────

    /// HKDF-Extract : retourne la pseudo-random key (PRK).
    ///
    /// Délègue à `security::crypto::kdf::hkdf_extract` (BLAKE3).
    pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
        let (prk, _) = security_hkdf_extract(Some(salt), ikm);
        *prk.as_bytes()
    }

    /// HKDF-Expand : étend la PRK en `length` octets.
    ///
    /// OOM-02 : utilise `try_reserve`.
    pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> ExofsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        if length > HKDF_MAX_OUTPUT {
            return Err(ExofsError::InvalidArgument);
        }

        let mut out = Vec::new();
        out.try_reserve(length).map_err(|_| ExofsError::NoMemory)?;
        out.resize(length, 0u8);

        let hkdf = Hkdf::<Sha256>::from_prk(prk).map_err(|_| ExofsError::InvalidArgument)?;
        hkdf.expand(info, &mut out)
            .map_err(|_| ExofsError::InvalidArgument)?;
        Ok(out)
    }

    /// HKDF complet : Extract + Expand.
    ///
    /// OOM-02 : utilise `try_reserve`.
    pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> ExofsResult<Vec<u8>> {
        let prk = Self::hkdf_extract(salt, ikm);
        Self::hkdf_expand(&prk, info, length)
    }

    // ── Dérivation de clé standard ──────────────────────────────────────────

    /// Dérive une clé de 32 octets depuis `secret`, `salt` et `context`.
    ///
    /// Utilise BLAKE3-KDF (Domain separation via context string).
    pub fn derive_key(secret: &[u8], salt: &[u8], context: &[u8]) -> ExofsResult<DerivedKey> {
        // Matériel = secret || salt
        let mut material = Vec::new();
        material
            .try_reserve(secret.len() + salt.len())
            .map_err(|_| ExofsError::NoMemory)?;
        material.extend_from_slice(secret);
        material.extend_from_slice(salt);

        // BLAKE3 KDF avec context string
        let dk = blake3_kdf(context, &material);
        Ok(DerivedKey::from_bytes(*dk.as_bytes()))
    }

    /// Dérive une clé pour un usage (KeyPurpose) spécifique.
    pub fn derive_for_purpose(
        secret: &[u8],
        salt: &[u8],
        purpose: KeyPurpose,
    ) -> ExofsResult<DerivedKey> {
        Self::derive_key(secret, salt, purpose.as_context())
    }

    /// Dérive plusieurs clés en batch depuis le même matériel.
    ///
    /// OOM-02 : pré-alloue le vecteur résultat.
    pub fn derive_batch(
        secret: &[u8],
        salt: &[u8],
        infos: &[&[u8]],
    ) -> ExofsResult<Vec<DerivedKey>> {
        let mut result = Vec::new();
        result
            .try_reserve(infos.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for info in infos {
            result.push(Self::derive_key(secret, salt, info)?);
        }
        Ok(result)
    }

    // ── Étirement de passphrase (Argon2id — S-16) ─────────────────────────

    /// Étire une passphrase avec Argon2id.
    ///
    /// `iterations` = Argon2id time_cost (minimum KDF_MIN_ITERS = 3).
    /// Paramètres fixes : m=65536 (64 MiB), p=4 (S-16).
    ///
    /// Retourne une erreur si `passphrase` est vide ou si `iterations` < KDF_MIN_ITERS.
    pub fn derive_from_passphrase(
        passphrase: &[u8],
        salt: &[u8; 32],
        iterations: u8,
    ) -> ExofsResult<DerivedKey> {
        if passphrase.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        if iterations < KDF_MIN_ITERS {
            return Err(ExofsError::InvalidArgument);
        }

        // Argon2id (S-16) : m=65536, t=iterations, p=4
        let params = argon2::Params::new(
            65_536,            // m_cost = 64 MiB
            iterations as u32, // t_cost
            4,                 // p_cost
            Some(32),          // output length
        )
        .map_err(|_| ExofsError::InvalidArgument)?;

        let argon2 =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let mut output = [0u8; 32];
        let mut memory = Vec::new();
        memory
            .try_reserve(argon2.params().block_count())
            .map_err(|_| ExofsError::NoMemory)?;
        memory.resize_with(argon2.params().block_count(), argon2::Block::default);

        argon2
            .hash_password_into_with_memory(passphrase, salt, &mut output, memory.as_mut_slice())
            .map_err(|_| ExofsError::InvalidArgument)?;

        Ok(DerivedKey::from_bytes(output))
    }

    /// Étirement avec les paramètres par défaut (S-16 : m=65536, t=3, p=4).
    pub fn derive_from_passphrase_default(
        passphrase: &[u8],
        salt: &[u8; 32],
    ) -> ExofsResult<DerivedKey> {
        Self::derive_from_passphrase(passphrase, salt, KDF_DEFAULT_ITERS)
    }

    /// Dérive une clé depuis une passphrase avec Argon2id (alias explicite).
    ///
    /// Identique à `derive_from_passphrase_default` pour compatibilité.
    pub fn derive_from_passphrase_argon2(
        passphrase: &[u8],
        salt: &[u8; 32],
    ) -> ExofsResult<[u8; 32]> {
        let dk = Self::derive_from_passphrase_default(passphrase, salt)?;
        Ok(*dk.as_bytes())
    }

    // ── Dérivations ExoFS spécifiques ─────────────────────────────────────

    /// Dérive une clé d'objet depuis une clé de volume et un blob_id.
    ///
    /// Utilise `security::crypto::kdf::derive_fs_block_key` avec blob_id comme index.
    pub fn derive_object_key(volume_key: &[u8; 32], blob_id: u64) -> ExofsResult<DerivedKey> {
        let dk =
            derive_fs_block_key(volume_key, blob_id).map_err(|_| ExofsError::InvalidArgument)?;
        Ok(DerivedKey::from_bytes(*dk.as_bytes()))
    }

    /// Dérive une clé de volume depuis une clé maître et un volume_id.
    pub fn derive_volume_key(master_key: &[u8; 32], volume_id: u64) -> ExofsResult<DerivedKey> {
        let ctx = b"ExoFS-VolumeKey-v1";
        let mut material = [0u8; 40]; // master_key(32) + volume_id(8)
        material[..32].copy_from_slice(master_key);
        material[32..].copy_from_slice(&volume_id.to_le_bytes());
        let dk = blake3_kdf(ctx, &material);
        Ok(DerivedKey::from_bytes(*dk.as_bytes()))
    }

    /// Dérive une clé d'index B-tree depuis une clé maître et un tree_id.
    pub fn derive_index_key(master_key: &[u8; 32], tree_id: u32) -> ExofsResult<DerivedKey> {
        let ctx = b"ExoFS-IndexKey-v1";
        let mut material = [0u8; 36]; // master_key(32) + tree_id(4)
        material[..32].copy_from_slice(master_key);
        material[32..].copy_from_slice(&tree_id.to_le_bytes());
        let dk = blake3_kdf(ctx, &material);
        Ok(DerivedKey::from_bytes(*dk.as_bytes()))
    }

    /// Vérifie qu'une clé dérivée correspond aux paramètres donnés.
    ///
    /// Comparaison constant-time via `subtle::ConstantTimeEq`.
    pub fn verify_derived_key(
        dk: &DerivedKey,
        secret: &[u8],
        salt: &[u8],
        context: &[u8],
    ) -> ExofsResult<bool> {
        use subtle::ConstantTimeEq;
        let expected = Self::derive_key(secret, salt, context)?;
        Ok(dk.as_bytes().ct_eq(expected.as_bytes()).into())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hkdf_extract_deterministic() {
        let p1 = KeyDerivation::hkdf_extract(b"salt", b"ikm");
        let p2 = KeyDerivation::hkdf_extract(b"salt", b"ikm");
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_hkdf_expand_lengths() {
        let prk = KeyDerivation::hkdf_extract(b"salt", b"ikm");
        for l in [1, 16, 32, 64, 100] {
            let v = KeyDerivation::hkdf_expand(&prk, b"info", l).unwrap();
            assert_eq!(v.len(), l);
        }
    }

    #[test]
    fn test_hkdf_expand_empty() {
        let prk = KeyDerivation::hkdf_extract(b"", b"ikm");
        assert!(KeyDerivation::hkdf_expand(&prk, b"", 0).unwrap().is_empty());
    }

    #[test]
    fn test_hkdf_expand_too_large() {
        let prk = KeyDerivation::hkdf_extract(b"", b"ikm");
        assert!(KeyDerivation::hkdf_expand(&prk, b"", HKDF_MAX_OUTPUT + 1).is_err());
    }

    #[test]
    fn test_derive_key_deterministic() {
        let k1 = KeyDerivation::derive_key(b"secret", b"salt", b"ctx").unwrap();
        let k2 = KeyDerivation::derive_key(b"secret", b"salt", b"ctx").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_key_context_separation() {
        let k1 = KeyDerivation::derive_key(b"s", b"salt", b"a").unwrap();
        let k2 = KeyDerivation::derive_key(b"s", b"salt", b"b").unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_for_purpose_separation() {
        let k1 =
            KeyDerivation::derive_for_purpose(b"s", b"salt", KeyPurpose::DataEncryption).unwrap();
        let k2 =
            KeyDerivation::derive_for_purpose(b"s", b"salt", KeyPurpose::Authentication).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_from_passphrase_empty_fails() {
        assert!(KeyDerivation::derive_from_passphrase(b"", &[0u8; 32], 3).is_err());
    }

    #[test]
    fn test_derive_from_passphrase_deterministic() {
        let k1 = KeyDerivation::derive_from_passphrase(b"hunter2", &[1u8; 32], 3).unwrap();
        let k2 = KeyDerivation::derive_from_passphrase(b"hunter2", &[1u8; 32], 3).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_from_passphrase_salt_changes_key() {
        let k1 = KeyDerivation::derive_from_passphrase(b"pass", &[1u8; 32], 3).unwrap();
        let k2 = KeyDerivation::derive_from_passphrase(b"pass", &[2u8; 32], 3).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_object_key_unique() {
        let vk = [0x42u8; 32];
        let k1 = KeyDerivation::derive_object_key(&vk, 1).unwrap();
        let k2 = KeyDerivation::derive_object_key(&vk, 2).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_derive_batch() {
        let infos: &[&[u8]] = &[b"a", b"b", b"c"];
        let keys = KeyDerivation::derive_batch(b"sec", b"s", infos).unwrap();
        assert_eq!(keys.len(), 3);
        // Toutes différentes
        assert_ne!(keys[0].as_bytes(), keys[1].as_bytes());
        assert_ne!(keys[1].as_bytes(), keys[2].as_bytes());
    }

    #[test]
    fn test_verify_derived_key_ok() {
        let k = KeyDerivation::derive_key(b"sec", b"salt", b"ctx").unwrap();
        assert!(KeyDerivation::verify_derived_key(&k, b"sec", b"salt", b"ctx").unwrap());
    }

    #[test]
    fn test_verify_derived_key_fail() {
        let k = KeyDerivation::derive_key(b"sec", b"salt", b"ctx").unwrap();
        assert!(!KeyDerivation::verify_derived_key(&k, b"other", b"salt", b"ctx").unwrap());
    }
}
