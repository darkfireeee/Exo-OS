//! SecretReader — déchiffrement de blobs ExoFS (no_std).
//!
//! Reconstruit la ObjectKey depuis la VolumeKey + le BlobId du header on-disk,
//! puis déchiffre avec XChaCha20-Poly1305.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::object_key::{ObjectKey, EncryptedBlobHeader, ENCRYPTED_BLOB_MAGIC};
use super::xchacha20::{Nonce, Tag, XChaCha20Key, XChaCha20Poly1305};

/// Lecteur de blobs chiffrés.
pub struct SecretReader {
    volume_key: super::volume_key::VolumeKey,
}

impl SecretReader {
    pub fn new(volume_key: super::volume_key::VolumeKey) -> Self {
        Self { volume_key }
    }

    /// Déchiffre un blob à partir du header et du ciphertext séparés.
    pub fn read_blob(
        &self,
        header: &EncryptedBlobHeader,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, FsError> {
        // RÈGLE 8 : vérifier le magic en premier.
        if header.magic != ENCRYPTED_BLOB_MAGIC {
            return Err(FsError::InvalidMagic);
        }

        let blob_id = BlobId::from_raw(header.blob_id);
        let okey = ObjectKey::derive(&self.volume_key, &blob_id);
        let xchacha_key: XChaCha20Key = okey.as_xchacha20_key();
        let nonce = Nonce(header.nonce);
        let tag   = Tag(header.tag);

        XChaCha20Poly1305::decrypt(&xchacha_key, &nonce, aad, ciphertext, &tag)
    }

    /// Déchiffre depuis un buffer sérialisé (header || ciphertext contigus).
    pub fn read_blob_serialized(
        &self,
        data: &[u8],
        aad: &[u8],
    ) -> Result<(BlobId, Vec<u8>), FsError> {
        if data.len() < 80 {
            return Err(FsError::InvalidData);
        }
        let header_bytes: &[u8; 80] = data[..80].try_into().map_err(|_| FsError::InvalidData)?;
        let header = EncryptedBlobHeader::from_bytes(header_bytes)?;
        let ciphertext = &data[80..];
        let blob_id = BlobId::from_raw(header.blob_id);
        let plaintext = self.read_blob(&header, ciphertext, aad)?;

        // Vérifie l'intégrité : BlobId doit correspondre aux données déchiffrées (RÈGLE 11).
        let computed_id = BlobId::from_bytes_blake3(&plaintext);
        if computed_id != blob_id {
            return Err(FsError::IntegrityCheckFailed);
        }

        Ok((blob_id, plaintext))
    }

    /// Déchiffre un lot de blobs sérialisés.
    pub fn read_blobs_batch(
        &self,
        serialized_blobs: &[Vec<u8>],
        aad: &[u8],
    ) -> Result<Vec<(BlobId, Vec<u8>)>, FsError> {
        let mut out = Vec::new();
        out.try_reserve(serialized_blobs.len()).map_err(|_| FsError::OutOfMemory)?;
        for buf in serialized_blobs {
            out.push(self.read_blob_serialized(buf, aad)?);
        }
        Ok(out)
    }

    /// Tente de déchiffrer sans vérifier le BlobId (mode récupération).
    pub fn read_blob_unchecked(
        &self,
        header: &EncryptedBlobHeader,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, FsError> {
        if header.magic != ENCRYPTED_BLOB_MAGIC {
            return Err(FsError::InvalidMagic);
        }
        let blob_id = BlobId::from_raw(header.blob_id);
        let okey = ObjectKey::derive(&self.volume_key, &blob_id);
        let xchacha_key = okey.as_xchacha20_key();
        let nonce = Nonce(header.nonce);
        let tag   = Tag(header.tag);
        XChaCha20Poly1305::decrypt(&xchacha_key, &nonce, aad, ciphertext, &tag)
    }

    pub fn volume_key(&self) -> &super::volume_key::VolumeKey {
        &self.volume_key
    }
}
