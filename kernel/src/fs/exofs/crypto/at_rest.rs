//! at_rest.rs — Chiffrement-at-rest ExoFS (FIX-F1).
//!
//! ## Architecture (extensible — « le mieux pour le projet et le futur »)
//!
//! ```text
//!   KekSource (passphrase | TPM | SecureBoot)
//!        │  derive_kek()
//!        ▼
//!   KEK ──wrap/unwrap (XChaCha20-Poly1305 AEAD)──► Volume Key (VK, aléatoire)
//!        stockée wrappée dans le superblock (_pad1)
//!        ▼  install au montage : volume_secret::set_volume_key(VK)
//!   VK ──derive_key(VK, blob_id)──► clé de blob
//!        + nonce = BLAKE3(blob_id || offset)
//!        ▼  xor_block() (XChaCha20 flux, longueur-préservante)
//!   Blob chiffré sur disque
//! ```
//!
//! ## Choix de conception
//!
//! - **Provider de KEK abstrait** : aujourd'hui `Passphrase` (Argon2id, aucun
//!   matériel) ; `TpmSealed` / `SecureBootSealed` sont des **points d'extension**
//!   documentés (renvoient `NotSupported` tant qu'aucun pilote n'existe — échec
//!   HONNÊTE, jamais de fausse sécurité). Un TPM complet (TIS + PCR policy) viendra
//!   se brancher ici sans toucher le reste.
//! - **Chiffrement de flux longueur-préservant** au niveau blob : pas d'expansion
//!   de tag → **aucune migration du format disque**. Sûr car les blobs ExoFS sont
//!   **immuables et adressés par contenu** : une paire (clé, nonce) ne chiffre
//!   qu'un seul plaintext. L'intégrité reste assurée par le checksum BLAKE3 de blob.
//! - **Gated** : sans clé de volume installée, tout est inactif (chemin en clair
//!   inchangé) — zéro régression sur les volumes non chiffrés.

use super::entropy::ENTROPY_POOL;
use super::volume_secret;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// FIX-F1 : toute la crypto est déléguée au crate **partagé** `exo-fscrypt`
// (source unique de vérité kernel↔mkfs). Plus aucune primitive crypto locale ici.

/// Mappe une erreur `exo-fscrypt` vers `ExofsError`.
fn map_fscrypt(e: exo_fscrypt::FsCryptError) -> ExofsError {
    match e {
        exo_fscrypt::FsCryptError::EmptyPassphrase => ExofsError::InvalidArgument,
        exo_fscrypt::FsCryptError::KdfFailure => ExofsError::InternalError,
        exo_fscrypt::FsCryptError::BadFormat => ExofsError::InvalidMagic,
        exo_fscrypt::FsCryptError::AuthFailed => ExofsError::PermissionDenied,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider de KEK
// ─────────────────────────────────────────────────────────────────────────────

/// Source de la KEK (Key Encryption Key) qui protège la clé de volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KekSource {
    /// KEK = Argon2id(passphrase, salt). Aucun matériel requis. **Implémenté.**
    Passphrase = 0,
    /// KEK descellée d'un TPM contre une politique PCR. **Point d'extension.**
    TpmSealed = 1,
    /// KEK dérivée d'une mesure Secure Boot scellée. **Point d'extension.**
    SecureBootSealed = 2,
}

impl KekSource {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Passphrase),
            1 => Some(Self::TpmSealed),
            2 => Some(Self::SecureBootSealed),
            _ => None,
        }
    }
}

/// Dérive la KEK 32 octets depuis la source configurée.
///
/// - `Passphrase` : Argon2id (paramètres S-16) sur `(material, salt)`.
/// - `TpmSealed` / `SecureBootSealed` : renvoient `NotSupported` tant qu'aucun
///   pilote TPM / mécanisme de scellé n'est câblé (échec honnête).
pub fn derive_kek(source: KekSource, material: &[u8], salt: &[u8; 32]) -> ExofsResult<[u8; 32]> {
    match source {
        KekSource::Passphrase => exo_fscrypt::derive_kek(material, salt).map_err(map_fscrypt),
        KekSource::TpmSealed | KekSource::SecureBootSealed => {
            // SEAM : brancher ici le descellement TPM (PCR policy) ou la dérivation
            // depuis une mesure Secure Boot. Aucun pilote disponible → REFUS.
            Err(ExofsError::NotSupported)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrap / unwrap de la clé de volume (AEAD authentifié — stocké dans le superblock)
// ─────────────────────────────────────────────────────────────────────────────

/// Taille sérialisée d'une clé de volume wrappée (format `exo-fscrypt`, 110 o),
/// destinée au superblock (`_pad1[272]`).
pub const WRAPPED_VK_LEN: usize = exo_fscrypt::WRAPPED_VK_LEN;

/// Wrappe une clé de volume `vk` (délègue à `exo-fscrypt`). Le salt et le nonce
/// sont tirés de l'entropie kernel. `TpmSealed`/`SecureBootSealed` → `NotSupported`.
pub fn wrap_volume_key(
    vk: &[u8; 32],
    source: KekSource,
    material: &[u8],
) -> ExofsResult<[u8; WRAPPED_VK_LEN]> {
    match source {
        KekSource::Passphrase => {
            let salt = ENTROPY_POOL.random_32();
            let nonce_full = ENTROPY_POOL.random_32();
            let mut nonce = [0u8; 24];
            nonce.copy_from_slice(&nonce_full[..24]);
            exo_fscrypt::wrap_volume_key(vk, material, &salt, &nonce).map_err(map_fscrypt)
        }
        KekSource::TpmSealed | KekSource::SecureBootSealed => Err(ExofsError::NotSupported),
    }
}

/// Déwrappe une clé de volume (délègue à `exo-fscrypt`). Échoue (auth AEAD) si la
/// passphrase est incorrecte ou les données altérées.
pub fn unwrap_volume_key(wrapped: &[u8], material: &[u8]) -> ExofsResult<[u8; 32]> {
    exo_fscrypt::unwrap_volume_key(wrapped, material).map_err(map_fscrypt)
}

/// FIX-F1 : point d'entrée de **déverrouillage au montage**. Déwrappe la clé de
/// volume (depuis le superblock) avec la `passphrase`, puis l'installe globalement
/// pour activer le chiffrement-at-rest des blobs.
///
/// À appeler par la séquence de montage SI `superblock.is_encrypted()` :
/// ```ignore
/// if sb.is_encrypted() {
///     if let Some(w) = sb.wrapped_volume_key() {
///         at_rest::install_volume_key_from_wrapped(&w, boot_passphrase)?;
///     }
/// }
/// ```
/// La source de la `passphrase` (paramètre de boot / scellé TPM) est la dernière
/// étape de câblage — documentée dans `docs/SECURITE/AUDIT-100-PERCENT.md` (F1).
pub fn install_volume_key_from_wrapped(wrapped: &[u8], passphrase: &[u8]) -> ExofsResult<()> {
    let vk = unwrap_volume_key(wrapped, passphrase)?;
    volume_secret::set_volume_key(vk);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Chiffrement de blob au niveau bloc (longueur-préservant)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la clé de chiffrement d'un blob (dérivée de la clé de volume + BlobId),
/// ou `None` si aucun volume chiffré n'est monté (→ chemin en clair, pas de fausse
/// sécurité). C'est le **point de gating** du chiffrement-at-rest.
pub fn blob_at_rest_key(blob_id: &BlobId) -> Option<[u8; 32]> {
    let vk = volume_secret::volume_key()?;
    Some(exo_fscrypt::blob_key(&vk, &blob_id.0))
}

/// Chiffre/déchiffre `buf` en place (involution) pour le blob à `disk_offset`.
/// `key` provient de [`blob_at_rest_key`]. Longueur inchangée. Délègue à
/// `exo-fscrypt` (même code que mkfs → cohérence par construction).
#[inline]
pub fn xor_block(key: &[u8; 32], blob_id: &BlobId, disk_offset: u64, buf: &mut [u8]) {
    exo_fscrypt::xor_block(key, &blob_id.0, disk_offset, buf);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    #[test]
    fn kek_passphrase_deterministic_and_salt_sensitive() {
        let salt1 = [1u8; 32];
        let salt2 = [2u8; 32];
        let k1 = derive_kek(KekSource::Passphrase, b"hunter2", &salt1).unwrap();
        let k1b = derive_kek(KekSource::Passphrase, b"hunter2", &salt1).unwrap();
        let k2 = derive_kek(KekSource::Passphrase, b"hunter2", &salt2).unwrap();
        assert_eq!(k1, k1b);
        assert_ne!(k1, k2);
    }

    #[test]
    fn kek_empty_passphrase_rejected() {
        assert!(derive_kek(KekSource::Passphrase, b"", &[0u8; 32]).is_err());
    }

    #[test]
    fn kek_tpm_and_secureboot_not_supported_yet() {
        // Échec HONNÊTE tant que le pilote n'existe pas (pas de fausse sécurité).
        assert_eq!(
            derive_kek(KekSource::TpmSealed, b"x", &[0u8; 32]),
            Err(ExofsError::NotSupported)
        );
        assert_eq!(
            derive_kek(KekSource::SecureBootSealed, b"x", &[0u8; 32]),
            Err(ExofsError::NotSupported)
        );
    }

    #[test]
    fn volume_key_wrap_unwrap_roundtrip() {
        let vk = [0x5Au8; 32];
        let wrapped = wrap_volume_key(&vk, KekSource::Passphrase, b"correct horse").unwrap();
        let got = unwrap_volume_key(&wrapped, b"correct horse").unwrap();
        assert_eq!(got, vk);
    }

    #[test]
    fn volume_key_wrong_passphrase_fails_auth() {
        let vk = [0x5Au8; 32];
        let wrapped = wrap_volume_key(&vk, KekSource::Passphrase, b"correct").unwrap();
        assert!(unwrap_volume_key(&wrapped, b"wrong").is_err());
    }

    #[test]
    fn volume_key_tampered_ciphertext_fails_auth() {
        let vk = [0x5Au8; 32];
        let mut wrapped = wrap_volume_key(&vk, KekSource::Passphrase, b"pw").unwrap();
        wrapped[70] ^= 0xFF; // corrompre le ciphertext
        assert!(unwrap_volume_key(&wrapped, b"pw").is_err());
    }

    #[test]
    fn block_xor_roundtrip_is_identity() {
        // cfg(test) : volume_secret fournit une clé déterministe → blob_at_rest_key Some.
        let bid = blob(0xAB);
        let key = blob_at_rest_key(&bid).expect("clé de volume présente en test");
        let plain = b"ExoFS at-rest confidential payload, immutable blob.".to_vec();
        let mut buf = plain.clone();
        xor_block(&key, &bid, 4096, &mut buf);
        assert_ne!(buf, plain, "le chiffrement doit modifier les données");
        xor_block(&key, &bid, 4096, &mut buf); // involution
        assert_eq!(buf, plain, "decrypt doit restaurer le clair");
    }

    #[test]
    fn block_different_blobs_diverge() {
        let plain = [0xEEu8; 64];
        let ka = blob_at_rest_key(&blob(1)).unwrap();
        let kb = blob_at_rest_key(&blob(2)).unwrap();
        let mut a = plain;
        let mut b = plain;
        xor_block(&ka, &blob(1), 0, &mut a);
        xor_block(&kb, &blob(2), 0, &mut b);
        assert_ne!(a, b, "deux blobs distincts → ciphertext distinct");
    }

    #[test]
    fn block_different_offsets_diverge() {
        let bid = blob(7);
        let key = blob_at_rest_key(&bid).unwrap();
        let plain = [0x11u8; 64];
        let mut a = plain;
        let mut b = plain;
        xor_block(&key, &bid, 0, &mut a);
        xor_block(&key, &bid, 512, &mut b);
        assert_ne!(a, b, "deux offsets distincts → keystream distinct");
    }

    #[test]
    fn wrapped_vk_len_fits_superblock_reserve() {
        // Doit tenir dans le _pad1[272] du superblock.
        assert!(WRAPPED_VK_LEN <= 272);
    }
}
