//! SecretWriter — chiffrement de blobs ExoFS avec ObjectKey (no_std).
//!
//! RÈGLE 11 : BlobId = Blake3(données AVANT chiffrement ET AVANT compression.
//! Le SecretWriter reçoit les données brutes, calcule le BlobId, puis chiffre.
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::object_key::{ObjectKey, EncryptedBlobHeader};
use super::entropy::ENTROPY_POOL;

/// Résultat d'une écriture de blob chiffré.
pub struct SecretWriteResult {
    pub blob_id:    BlobId,      // BlobId calculé sur les données brutes (RÈGLE 11).
    pub header:     EncryptedBlobHeader,
    pub ciphertext: Vec<u8>,
    pub plaintext_len: usize,
}

/// Écrit un blob chiffré avec sa clé de volume.
pub struct SecretWriter {
    volume_key: super::volume_key::VolumeKey,
}

impl SecretWriter {
    pub fn new(volume_key: super::volume_key::VolumeKey) -> Self {
        Self { volume_key }
    }

    /// Chiffre `plaintext` :
    /// 1. BlobId = Blake3(plaintext) — RÈGLE 11.
    /// 2. Dérive ObjectKey depuis VolumeKey + BlobId.
    /// 3. Chiffre avec XChaCha20-Poly1305.
    /// 4. Retourne header + ciphertext.
    pub fn write_blob(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<SecretWriteResult, FsError> {
        // RÈGLE 11 : calcul du BlobId sur les données brutes.
        let blob_id = BlobId::from_bytes_blake3(plaintext);

        // Dérivation de la clé objet.
        let okey = ObjectKey::derive(&self.volume_key, &blob_id);

        // Nonce frais depuis l'EntropyPool.
        let nonce = ENTROPY_POOL.random_nonce();

        // Chiffrement XChaCha20-Poly1305.
        let xchacha_key = okey.as_xchacha20_key();
        let (ct, tag) = super::xchacha20::XChaCha20Poly1305::encrypt(
            &xchacha_key,
            &nonce,
            aad,
            plaintext,
        );

        let header = EncryptedBlobHeader::new(&blob_id, &nonce, &tag);

        Ok(SecretWriteResult {
            blob_id,
            header,
            ciphertext: ct,
            plaintext_len: plaintext.len(),
        })
    }

    /// Chiffre une tranche de données et sérialise le tout (header || ciphertext) dans un Vec.
    pub fn write_blob_serialized(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<(BlobId, Vec<u8>), FsError> {
        let result = self.write_blob(plaintext, aad)?;
        let header_bytes = result.header.to_bytes();
        let total = 80 + result.ciphertext.len();
        let mut out = Vec::new();
        out.try_reserve(total).map_err(|_| FsError::OutOfMemory)?;
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(&result.ciphertext);
        Ok((result.blob_id, out))
    }

    /// Chiffre plusieurs blobs indépendants en une seule passe.
    pub fn write_blobs_batch(
        &self,
        blobs: &[&[u8]],
        aad: &[u8],
    ) -> Result<Vec<SecretWriteResult>, FsError> {
        let mut out = Vec::new();
        out.try_reserve(blobs.len()).map_err(|_| FsError::OutOfMemory)?;
        for b in blobs {
            out.push(self.write_blob(b, aad)?);
        }
        Ok(out)
    }

    /// Volume key associé à ce writer.
    pub fn volume_key(&self) -> &super::volume_key::VolumeKey {
        &self.volume_key
    }
}
