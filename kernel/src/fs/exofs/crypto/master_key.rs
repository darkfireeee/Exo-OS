//! Clé maître ExoFS — racine de la hiérarchie cryptographique.
//!
//! La `MasterKey` est la clé racine depuis laquelle toutes les autres clés
//! (volume, objet, session) sont dérivées. Elle doit être protégée avec
//! le niveau de sécurité le plus élevé et ne jamais être stockée en clair.
//!
//! # Cycle de vie
//! 1. Générer via `MasterKey::generate()` (entropie matérielle).
//! 2. Protéger via `MasterKey::wrap_with_passphrase()`.
//! 3. Restaurer via `MasterKey::unwrap_from_passphrase()`.
//! 4. Utiliser via `MasterKey::derive_*()`.
//! 5. Zeroize automatique au drop.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::entropy::ENTROPY_POOL;
use super::key_derivation::KeyDerivation;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::security::crypto::kdf;
use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use sha2::Sha256;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic ExoFS pour les enveloppes de clé maître.
pub const MASTER_KEY_MAGIC: u32 = 0xEF_4B_4D_53; // "EFKMS"
/// Taille d'une clé maître (256 bits).
pub const MASTER_KEY_LEN: usize = 32;
/// Taille de la structure WrappedMasterKey sérialisée.
pub const WRAPPED_MASTER_KEY_SIZE: usize = 4   // magic
  + 8   // key_id
  + 32  // sel
  + 32  // ciphertext (XOR-key)
  + 32; // HMAC-SHA256 d'intégrité

/// Version du protocole d'enveloppe.
pub const WRAPPING_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'une clé maître.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterKeyId(pub u64);

impl MasterKeyId {
    /// Génère un identifiant depuis l'entropie du système.
    pub fn generate() -> Self {
        Self(ENTROPY_POOL.random_u64())
    }
}

impl core::fmt::Display for MasterKeyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MasterKey({:#018x})", self.0)
    }
}

/// Clé maître (zeroize on drop).
///
/// 256 bits d'entropie identifiés par un `MasterKeyId`.
/// Toutes les dérivations de clés doivent utiliser cette clé comme IKM.
pub struct MasterKey {
    /// Matériel de clé (zeroize on drop).
    key: [u8; MASTER_KEY_LEN],
    /// Identifiant stable.
    id: MasterKeyId,
}

impl core::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MasterKey {{ id: {:?}, key: <redacted> }}", self.id)
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
    }
}

/// Clé maître encapsulée (en clair = forme transportable chiffrée).
#[derive(Debug, Clone)]
pub struct WrappedMasterKey {
    /// Magic d'identification.
    pub magic: u32,
    /// Identifiant de la clé emballée.
    pub key_id: MasterKeyId,
    /// Sel aléatoire utilisé pour la dérivation de la KEK.
    pub salt: [u8; 32],
    /// Texte chiffré (XOR avec la KEK, simple en no_std).
    pub ciphertext: [u8; MASTER_KEY_LEN],
    /// Tag HMAC-SHA256 sur magic || key_id || salt || ciphertext.
    pub mac: [u8; 32],
}

/// Métadonnées publiques d'une clé maître (sans le matériel secret).
#[derive(Debug, Clone)]
pub struct MasterKeyMetadata {
    pub id: MasterKeyId,
    pub created_at: u64,
    pub version: u8,
}

impl MasterKey {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    /// Génère une nouvelle clé maître depuis l'entropie matérielle.
    pub fn generate() -> ExofsResult<Self> {
        let raw = ENTROPY_POOL.random_bytes(MASTER_KEY_LEN)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self {
            key,
            id: MasterKeyId::generate(),
        })
    }

    /// Génère depuis un identifiant fourni (pour restauration).
    pub fn generate_with_id(id: MasterKeyId) -> ExofsResult<Self> {
        let raw = ENTROPY_POOL.random_bytes(MASTER_KEY_LEN)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self { key, id })
    }

    /// Construit depuis des bytes existants (import de clé).
    ///
    /// SECURITY: Le matériel entrant doit provenir d'une source de confiance.
    pub fn from_bytes(key_bytes: [u8; MASTER_KEY_LEN], id: MasterKeyId) -> Self {
        Self { key: key_bytes, id }
    }

    /// Dérive depuis une passphrase + sel (enrôlement initial).
    pub fn derive_from_passphrase(passphrase: &[u8], salt: &[u8; 32]) -> ExofsResult<Self> {
        let dk = KeyDerivation::derive_from_passphrase_default(passphrase, salt)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(dk.as_bytes());
        Ok(Self {
            key,
            id: MasterKeyId::generate(),
        })
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    /// Retourne l'identifiant.
    pub fn id(&self) -> MasterKeyId {
        self.id
    }

    /// Expose le matériel brut pour la dérivation (référence courte durée).
    pub fn raw_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.key
    }

    /// Métadonnées publiques.
    pub fn metadata(&self) -> MasterKeyMetadata {
        MasterKeyMetadata {
            id: self.id,
            created_at: ENTROPY_POOL.random_u64(),
            version: WRAPPING_VERSION,
        }
    }

    // ── Dérivation ───────────────────────────────────────────────────────────

    /// Dérive une clé de volume depuis cette clé maître et un identifiant de volume.
    pub fn derive_volume_key(&self, volume_id: u64) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_volume_key(&self.key, volume_id)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive une clé d'index.
    pub fn derive_index_key(&self, tree_id: u32) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_index_key(&self.key, tree_id)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive une clé générique avec un contexte.
    pub fn derive_key_for_context(&self, context: &[u8]) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_key(&self.key, b"", context)?;
        Ok(*dk.as_bytes())
    }

    // ── Wrapping / Unwrapping ─────────────────────────────────────────────────

    /// Enveloppe la clé maître avec une passphrase (KEK dérivée HKDF).
    ///
    /// L'enveloppe contient : magic, key_id, salt, ciphertext, HMAC.
    pub fn wrap_with_passphrase(&self, passphrase: &[u8]) -> ExofsResult<WrappedMasterKey> {
        let salt_raw = ENTROPY_POOL.random_bytes(32)?;
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_raw);

        let (enc_key, mac_key) = derive_wrap_keys(passphrase, &salt)?;

        // Chiffrement XOR (simple en no_std ; à remplacer par AES-256-KW en production).
        let mut ciphertext = [0u8; MASTER_KEY_LEN];
        for i in 0..MASTER_KEY_LEN {
            ciphertext[i] = self.key[i] ^ enc_key[i];
        }

        // HMAC-SHA256 sur magic || key_id || salt || ciphertext.
        let mac = compute_wrap_mac(MASTER_KEY_MAGIC, self.id, &salt, &ciphertext, &mac_key)?;

        Ok(WrappedMasterKey {
            magic: MASTER_KEY_MAGIC,
            key_id: self.id,
            salt,
            ciphertext,
            mac,
        })
    }

    /// Déenveloppe une clé maître depuis une passphrase.
    pub fn unwrap_from_passphrase(
        wrapped: &WrappedMasterKey,
        passphrase: &[u8],
    ) -> ExofsResult<Self> {
        if wrapped.magic != MASTER_KEY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }

        let (enc_key, mac_key) = derive_wrap_keys(passphrase, &wrapped.salt)?;

        // Vérification du MAC.
        let expected_mac = compute_wrap_mac(
            wrapped.magic,
            wrapped.key_id,
            &wrapped.salt,
            &wrapped.ciphertext,
            &mac_key,
        )?;
        if !constant_time_eq_32(&expected_mac, &wrapped.mac) {
            return Err(ExofsError::CorruptedStructure);
        }

        // Déchiffrement.
        let mut key = [0u8; MASTER_KEY_LEN];
        for i in 0..MASTER_KEY_LEN {
            key[i] = wrapped.ciphertext[i] ^ enc_key[i];
        }
        Ok(Self {
            key,
            id: wrapped.key_id,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

type HmacSha256 = Hmac<Sha256>;

fn derive_wrap_keys(passphrase: &[u8], salt: &[u8; 32]) -> ExofsResult<([u8; 32], [u8; 32])> {
    if passphrase.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let stretched = KeyDerivation::derive_from_passphrase_default(passphrase, salt)?;
    let (enc, mac) =
        kdf::derive_enc_mac_keys(stretched.as_bytes(), Some(b"ExoFS-master-key-wrap-v1"))
            .map_err(|_| ExofsError::InvalidArgument)?;
    Ok((*enc.as_bytes(), *mac.as_bytes()))
}

fn compute_wrap_mac(
    magic: u32,
    key_id: MasterKeyId,
    salt: &[u8; 32],
    ciphertext: &[u8; 32],
    mac_key: &[u8; 32],
) -> ExofsResult<[u8; 32]> {
    let mut data: Vec<u8> = Vec::new();
    data.try_reserve(4 + 8 + 32 + 32)
        .map_err(|_| ExofsError::NoMemory)?;
    data.extend_from_slice(&magic.to_le_bytes());
    data.extend_from_slice(&key_id.0.to_le_bytes());
    data.extend_from_slice(salt);
    data.extend_from_slice(ciphertext);

    let mut mac = HmacSha256::new_from_slice(mac_key).map_err(|_| ExofsError::InvalidArgument)?;
    mac.update(&data);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&tag);
    Ok(out)
}

fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut d = 0u8;
    for i in 0..32 {
        d |= a[i] ^ b[i];
    }
    d == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ok() {
        let mk = MasterKey::generate().test_unwrap();
        assert_ne!(*mk.raw_bytes(), [0u8; 32]);
    }

    #[test]
    fn test_generate_different_each_time() {
        let a = MasterKey::generate().test_unwrap();
        let b = MasterKey::generate().test_unwrap();
        assert_ne!(a.raw_bytes(), b.raw_bytes());
    }

    #[test]
    fn test_id_stable() {
        let mk = MasterKey::generate().test_unwrap();
        assert_eq!(mk.id(), mk.id());
    }

    #[test]
    fn test_derive_from_passphrase_ok() {
        let mk = MasterKey::derive_from_passphrase(b"secret", &[1u8; 32]).test_unwrap();
        assert_eq!(mk.raw_bytes().len(), 32);
    }

    #[test]
    fn test_wrap_unwrap_roundtrip() {
        let mk = MasterKey::generate().test_unwrap();
        let orig = *mk.raw_bytes();
        let wrapped = mk.wrap_with_passphrase(b"passphrase").test_unwrap();
        let mk2 = MasterKey::unwrap_from_passphrase(&wrapped, b"passphrase").test_unwrap();
        assert_eq!(*mk2.raw_bytes(), orig);
    }

    #[test]
    fn test_wrap_uses_separate_enc_and_mac_keys() {
        let (enc, mac) = derive_wrap_keys(b"passphrase", &[0x5Au8; 32]).test_unwrap();
        assert_ne!(enc, mac);
    }

    #[test]
    fn test_wrap_wrong_passphrase_fails() {
        let mk = MasterKey::generate().test_unwrap();
        let wrapped = mk.wrap_with_passphrase(b"correct").test_unwrap();
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"wrong").is_err());
    }

    #[test]
    fn test_wrap_wrong_magic_fails() {
        let mk = MasterKey::generate().test_unwrap();
        let mut wrapped = mk.wrap_with_passphrase(b"pass").test_unwrap();
        wrapped.magic = 0xDEAD_BEEF;
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"pass").is_err());
    }

    #[test]
    fn test_wrap_tampered_ct_fails() {
        let mk = MasterKey::generate().test_unwrap();
        let mut wrapped = mk.wrap_with_passphrase(b"pass").test_unwrap();
        wrapped.ciphertext[0] ^= 0xFF;
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"pass").is_err());
    }

    #[test]
    fn test_derive_volume_key_ok() {
        let mk = MasterKey::generate().test_unwrap();
        let vk = mk.derive_volume_key(1).test_unwrap();
        assert_eq!(vk.len(), 32);
    }

    #[test]
    fn test_derive_volume_different_ids() {
        let mk = MasterKey::generate().test_unwrap();
        let vk1 = mk.derive_volume_key(1).test_unwrap();
        let vk2 = mk.derive_volume_key(2).test_unwrap();
        assert_ne!(vk1, vk2);
    }

    #[test]
    fn test_derive_index_key_ok() {
        let mk = MasterKey::generate().test_unwrap();
        let ik = mk.derive_index_key(42).test_unwrap();
        assert_eq!(ik.len(), 32);
    }

    #[test]
    fn test_master_key_id_display() {
        let id = MasterKeyId(0x1234);
        assert!(format!("{id}").contains("MasterKey"));
    }

    #[test]
    fn test_from_bytes_roundtrip() {
        let kb = [0xABu8; 32];
        let id = MasterKeyId::generate();
        let mk = MasterKey::from_bytes(kb, id);
        assert_eq!(*mk.raw_bytes(), kb);
    }
}
