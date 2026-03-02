//! ObjectKey — clé par-objet dérivée depuis la VolumeKey et le BlobId (no_std).
//!
//! BlobId = Blake3(données AVANT chiffrement ET compression — RÈGLE 11.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use crate::fs::exofs::core::{BlobId, FsError};
use super::key_derivation::DerivedKey;

/// Clé par-objet (256-bit) liée à un BlobId spécifique.
pub struct ObjectKey {
    derived:  DerivedKey,
    blob_id:  BlobId,
}

impl ObjectKey {
    /// Dérive une ObjectKey depuis la VolumeKey et le BlobId.
    ///
    /// RÈGLE 11 : le BlobId doit avoir été calculé sur les données BRUTES
    /// (avant compression et avant chiffrement).
    pub fn derive(
        volume_key: &super::volume_key::VolumeKey,
        blob_id: &BlobId,
    ) -> Self {
        let derived = volume_key.derive_object_key(blob_id);
        Self { derived, blob_id: *blob_id }
    }

    /// Retourne le BlobId associé.
    pub fn blob_id(&self) -> &BlobId {
        &self.blob_id
    }

    /// Retourne les bytes de la clé (usage interne pour XChaCha20).
    pub fn as_xchacha20_key(&self) -> super::xchacha20::XChaCha20Key {
        super::xchacha20::XChaCha20Key(self.derived.bytes)
    }

    /// Chiffre un buffer avec la clé et un nonce frais de l'EntropyPool.
    pub fn encrypt_blob(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<EncryptedBlob, FsError> {
        let nonce = super::entropy::ENTROPY_POOL.random_nonce();
        let xk = self.as_xchacha20_key();
        let (ciphertext, tag) =
            super::xchacha20::XChaCha20Poly1305::encrypt(&xk, &nonce, aad, plaintext);
        Ok(EncryptedBlob {
            blob_id: self.blob_id,
            nonce,
            tag,
            ciphertext,
        })
    }

    /// Déchiffre un `EncryptedBlob` avec la clé courante.
    pub fn decrypt_blob(
        &self,
        enc: &EncryptedBlob,
        aad: &[u8],
    ) -> Result<alloc::vec::Vec<u8>, FsError> {
        if enc.blob_id != self.blob_id {
            return Err(FsError::InvalidArgument);
        }
        let xk = self.as_xchacha20_key();
        super::xchacha20::XChaCha20Poly1305::decrypt(
            &xk,
            &enc.nonce,
            aad,
            &enc.ciphertext,
            &enc.tag,
        )
    }
}

/// Blob chiffré avec son nonce et son tag d'authentification.
pub struct EncryptedBlob {
    pub blob_id:    BlobId,
    pub nonce:      super::xchacha20::Nonce,
    pub tag:        super::xchacha20::Tag,
    pub ciphertext: alloc::vec::Vec<u8>,
}

/// En-tête on-disk pour un blob chiffré (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EncryptedBlobHeader {
    pub magic:   u32,                       // 0x454E4300 = 'ENC\0'
    pub version: u32,
    pub blob_id: [u8; 32],                  // BlobId des données originales (RÈGLE 11)
    pub nonce:   [u8; 24],                  // XChaCha20 nonce
    pub tag:     [u8; 16],                  // Poly1305 tag
}

pub const ENCRYPTED_BLOB_MAGIC: u32 = 0x454E4300;

const _: () = assert!(core::mem::size_of::<EncryptedBlobHeader>() == 80);

impl EncryptedBlobHeader {
    pub fn new(blob_id: &BlobId, nonce: &super::xchacha20::Nonce, tag: &super::xchacha20::Tag) -> Self {
        Self {
            magic:   ENCRYPTED_BLOB_MAGIC,
            version: 1,
            blob_id: blob_id.as_bytes(),
            nonce:   nonce.0,
            tag:     tag.0,
        }
    }

    pub fn from_bytes(bytes: &[u8; 80]) -> Result<Self, FsError> {
        // RÈGLE 8 : vérifier le magic en premier.
        let magic = u32::from_le_bytes(bytes[0..4].try_into().map_err(|_| FsError::InvalidData)?);
        if magic != ENCRYPTED_BLOB_MAGIC {
            return Err(FsError::InvalidMagic);
        }
        // SAFETY: EncryptedBlobHeader est #[repr(C)] avec des champs POD, la taille est vérifiée.
        let hdr: Self = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) };
        Ok(hdr)
    }

    pub fn to_bytes(&self) -> [u8; 80] {
        // SAFETY: EncryptedBlobHeader est #[repr(C)] POD, taille 80 vérifiée.
        unsafe { core::mem::transmute_copy(self) }
    }
}
