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
use super::xchacha20::{Nonce, Tag, XChaCha20Key, XChaCha20Poly1305};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

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
  + 32  // sel (Argon2id)
  + 24  // nonce XChaCha20
  + 32  // ciphertext (XChaCha20-Poly1305)
  + 16; // tag AEAD (128 bits)

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
    /// Sel aléatoire pour la dérivation Argon2id de la KEK.
    pub salt: [u8; 32],
    /// Nonce XChaCha20 (192 bits), aléatoire par wrap.
    pub nonce: [u8; 24],
    /// Texte chiffré par XChaCha20-Poly1305 (AEAD).
    pub ciphertext: [u8; MASTER_KEY_LEN],
    /// Tag d'authentification AEAD (128 bits) sur magic||key_id (AAD) + ct.
    pub tag: [u8; 16],
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

    /// Enveloppe la clé maître avec une passphrase.
    ///
    /// FIX-F7 : KEK = Argon2id(passphrase, salt) ; chiffrement **AEAD
    /// XChaCha20-Poly1305** (site crypto unique audité) — remplace l'ancien
    /// XOR+HMAC bespoke. AAD = magic || key_id (lie les métadonnées au chiffré).
    pub fn wrap_with_passphrase(&self, passphrase: &[u8]) -> ExofsResult<WrappedMasterKey> {
        if passphrase.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        let salt_raw = ENTROPY_POOL.random_bytes(32)?;
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_raw);
        let nonce_raw = ENTROPY_POOL.random_bytes(24)?;
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&nonce_raw);

        let kek = KeyDerivation::derive_from_passphrase_default(passphrase, &salt)?;
        let key = XChaCha20Key(*kek.as_bytes());
        let aad = wrap_aad(MASTER_KEY_MAGIC, self.id);
        let (ct_vec, tag) = XChaCha20Poly1305::encrypt(&key, &Nonce(nonce), &aad, &self.key)?;
        if ct_vec.len() != MASTER_KEY_LEN {
            return Err(ExofsError::InternalError);
        }
        let mut ciphertext = [0u8; MASTER_KEY_LEN];
        ciphertext.copy_from_slice(&ct_vec);

        Ok(WrappedMasterKey {
            magic: MASTER_KEY_MAGIC,
            key_id: self.id,
            salt,
            nonce,
            ciphertext,
            tag: tag.0,
        })
    }

    /// Déenveloppe une clé maître depuis une passphrase.
    ///
    /// L'authentification AEAD échoue (`CorruptedStructure`) si la passphrase est
    /// fausse ou si l'enveloppe a été altérée.
    pub fn unwrap_from_passphrase(
        wrapped: &WrappedMasterKey,
        passphrase: &[u8],
    ) -> ExofsResult<Self> {
        if wrapped.magic != MASTER_KEY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let kek = KeyDerivation::derive_from_passphrase_default(passphrase, &wrapped.salt)?;
        let key = XChaCha20Key(*kek.as_bytes());
        let aad = wrap_aad(wrapped.magic, wrapped.key_id);
        let plain = XChaCha20Poly1305::decrypt(
            &key,
            &Nonce(wrapped.nonce),
            &aad,
            &wrapped.ciphertext,
            &Tag(wrapped.tag),
        )
        .map_err(|_| ExofsError::CorruptedStructure)?;
        if plain.len() != MASTER_KEY_LEN {
            return Err(ExofsError::CorruptedStructure);
        }
        let mut key_bytes = [0u8; MASTER_KEY_LEN];
        key_bytes.copy_from_slice(&plain);
        Ok(Self {
            key: key_bytes,
            id: wrapped.key_id,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

/// AAD du wrap : magic (4) || key_id (8). Lie les métadonnées publiques au
/// chiffré (empêche le rejeu/échange d'enveloppes).
fn wrap_aad(magic: u32, key_id: MasterKeyId) -> [u8; 12] {
    let mut aad = [0u8; 12];
    aad[..4].copy_from_slice(&magic.to_le_bytes());
    aad[4..12].copy_from_slice(&key_id.0.to_le_bytes());
    aad
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
    fn test_wrap_uses_fresh_salt_and_nonce_per_call() {
        // FIX-F7 : chaque wrap tire un salt + nonce aléatoires → deux wraps de la
        // MÊME clé produisent des enveloppes différentes (pas de réutilisation).
        let mk = MasterKey::generate().test_unwrap();
        let w1 = mk.wrap_with_passphrase(b"pw").test_unwrap();
        let w2 = mk.wrap_with_passphrase(b"pw").test_unwrap();
        assert_ne!(w1.salt, w2.salt);
        assert_ne!(w1.nonce, w2.nonce);
        assert_ne!(w1.ciphertext, w2.ciphertext);
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
