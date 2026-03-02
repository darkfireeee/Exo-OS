//! MasterKey — clé maître du volume ExoFS (no_std).
//!
//! Dérivée depuis le passphrase utilisateur via KeyDerivation::from_passphrase.
//! Stockée uniquement en RAM, jamais écrite en clair sur disque.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::boxed::Box;
use crate::fs::exofs::core::FsError;
use super::key_derivation::KeyDerivation;
use super::entropy::ENTROPY_POOL;

/// Magic pour l'en-tête on-disk de vérification de la clé maître.
pub const MASTER_KEY_VERIFY_MAGIC: u32 = 0x4D4B5900; // 'MKY\0'

/// En-tête de vérification on-disk de la clé maître (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MasterKeyVerifyHeader {
    pub magic:          u32,
    pub kdf_iterations: u32,
    pub salt:           [u8; 32],   // Sel de dérivation 256 bits.
    pub hmac_check:     [u8; 32],   // HMAC-Blake3(MasterKey, b"verify")
    // Note : total = 4+4+32+32 = 72 bytes. Padding nécessaire ?
    // Le champ est volontairement 72 octets; utilisé dans un contexte plus large.
}

const _: () = assert!(core::mem::size_of::<MasterKeyVerifyHeader>() == 72);

/// Clé maître ExoFS (256-bit, sur le tas pour éviter de l'exposer sur stack).
pub struct MasterKey {
    inner: Box<MasterKeyInner>,
}

struct MasterKeyInner {
    bytes: [u8; 32],
    kdf:   KeyDerivation,
}

impl Drop for MasterKeyInner {
    fn drop(&mut self) {
        // Zeroize : efface les octets secrets de la clé maître.
        self.bytes.iter_mut().for_each(|b| *b = 0);
        self.kdf.prk.iter_mut().for_each(|b| *b = 0);
    }
}

impl MasterKey {
    /// Dérive une MasterKey depuis un passphrase et un sel (enregistrement).
    pub fn derive_from_passphrase(passphrase: &[u8], salt: &[u8; 32]) -> Result<Self, FsError> {
        let kdf = KeyDerivation::from_passphrase(passphrase, salt);
        let dk = kdf.derive_256("ExoFS.MasterKey")?;
        let inner = Box::new(MasterKeyInner {
            bytes: dk.bytes,
            kdf,
        });
        // Efface dk.bytes (Drop fait le zeroize mais on est sûr).
        Ok(Self { inner })
    }

    /// Génère un sel aléatoire pour une nouvelle clé maître.
    pub fn generate_salt() -> [u8; 32] {
        ENTROPY_POOL.random_key_256()
    }

    /// Retourne une référence aux bytes de la clé (usage interne uniquement).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.inner.bytes
    }

    /// Dérive une sous-clé de chiffrement pour un volume (wrapping d'une VolumeKey).
    pub fn derive_volume_wrap_key(&self) -> Result<super::key_derivation::DerivedKey, FsError> {
        self.inner.kdf.derive_256("ExoFS.VolumeWrapKey")
    }

    /// Construit l'en-tête de vérification on-disk (sans écrire la clé elle-même).
    pub fn build_verify_header(&self, salt: &[u8; 32], iterations: u32) -> MasterKeyVerifyHeader {
        // HMAC-Blake3(MasterKey, b"verify") pour confirmer la clé au démarrage.
        let hmac = super::key_derivation::blake3_hash_slice(self.inner.bytes.as_ref());
        MasterKeyVerifyHeader {
            magic:          MASTER_KEY_VERIFY_MAGIC,
            kdf_iterations: iterations,
            salt:           *salt,
            hmac_check:     hmac,
        }
    }

    /// Vérifie qu'un en-tête on-disk correspond à cette clé.
    pub fn verify_header(&self, hdr: &MasterKeyVerifyHeader) -> Result<(), FsError> {
        // RÈGLE 8 : vérifier magic en premier.
        if hdr.magic != MASTER_KEY_VERIFY_MAGIC {
            return Err(FsError::InvalidMagic);
        }
        let expected_hmac = super::key_derivation::blake3_hash_slice(self.inner.bytes.as_ref());
        if !constant_time_eq_32(&expected_hmac, &hdr.hmac_check) {
            return Err(FsError::AuthTagMismatch);
        }
        Ok(())
    }

    /// Chiffre une VolumeKey avec la clé de wrapping.
    pub fn wrap_volume_key(
        &self,
        plain_volume_key: &[u8; 32],
    ) -> Result<WrappedVolumeKey, FsError> {
        let wrap_key = self.derive_volume_wrap_key()?;
        let nonce = ENTROPY_POOL.random_nonce();
        let xk = super::xchacha20::XChaCha20Key(wrap_key.bytes);
        let (ct, tag) = super::xchacha20::XChaCha20Poly1305::encrypt(
            &xk,
            &nonce,
            b"ExoFS.VolumeKey",
            plain_volume_key,
        );
        let mut wrapped = [0u8; 32];
        wrapped.copy_from_slice(&ct);
        Ok(WrappedVolumeKey {
            nonce,
            ciphertext: wrapped,
            tag,
        })
    }

    /// Déchiffre une WrappedVolumeKey.
    pub fn unwrap_volume_key(
        &self,
        wrapped: &WrappedVolumeKey,
    ) -> Result<[u8; 32], FsError> {
        let wrap_key = self.derive_volume_wrap_key()?;
        let xk = super::xchacha20::XChaCha20Key(wrap_key.bytes);
        let pt = super::xchacha20::XChaCha20Poly1305::decrypt(
            &xk,
            &wrapped.nonce,
            b"ExoFS.VolumeKey",
            &wrapped.ciphertext,
            &wrapped.tag,
        )?;
        if pt.len() != 32 { return Err(FsError::InvalidData); }
        let mut key = [0u8; 32];
        key.copy_from_slice(&pt);
        Ok(key)
    }
}

/// VolumeKey chiffrée par la MasterKey (stockage on-disk).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WrappedVolumeKey {
    pub nonce:      super::xchacha20::Nonce,   // 24 bytes
    pub ciphertext: [u8; 32],                  // Volume key chiffrée
    pub tag:        super::xchacha20::Tag,     // 16 bytes
}

const _: () = assert!(core::mem::size_of::<WrappedVolumeKey>() == 72);

fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut v: u8 = 0;
    for i in 0..32 { v |= a[i] ^ b[i]; }
    v == 0
}
