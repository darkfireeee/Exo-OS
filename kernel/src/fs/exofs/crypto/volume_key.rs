//! VolumeKey — clé de volume ExoFS, chiffrée par la MasterKey on-disk (no_std).
//!
//! Chaque volume ExoFS possède une clé 256-bit aléatoire.
//! La VolumeKey en clair est uniquement en RAM.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::boxed::Box;
use crate::fs::exofs::core::FsError;
use super::entropy::ENTROPY_POOL;

/// Magic on-disk pour l'en-tête de VolumeKey.
pub const VOLUME_KEY_MAGIC: u32 = 0x564B4500; // 'VKE\0'

/// En-tête on-disk de VolumeKey chiffrée (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VolumeKeyHeader {
    pub magic:           u32,
    pub version:         u32,
    pub key_id:          u64,                              // Identifiant unique du volume
    pub wrapped_key:     super::master_key::WrappedVolumeKey, // 72 bytes
    pub created_epoch:   u64,
    pub _reserved:       [u8; 4],
}

const _: () = assert!(core::mem::size_of::<VolumeKeyHeader>() == 96);

/// VolumeKey en clair (uniquement en RAM).
pub struct VolumeKey {
    inner: Box<VolumeKeyInner>,
}

struct VolumeKeyInner {
    bytes:    [u8; 32],
    key_id:   u64,
    version:  u32,
}

impl Drop for VolumeKeyInner {
    fn drop(&mut self) {
        // Zeroize.
        self.bytes.iter_mut().for_each(|b| *b = 0);
    }
}

impl VolumeKey {
    /// Génère une nouvelle VolumeKey aléatoire.
    pub fn generate() -> Result<Self, FsError> {
        let bytes = ENTROPY_POOL.random_key_256();
        let key_id = {
            let mut id_bytes = [0u8; 8];
            ENTROPY_POOL.fill_bytes(&mut id_bytes);
            u64::from_le_bytes(id_bytes)
        };
        let inner = Box::new(VolumeKeyInner {
            bytes,
            key_id,
            version: 1,
        });
        Ok(Self { inner })
    }

    /// Reconstruit une VolumeKey depuis les bytes déchiffrés.
    pub fn from_raw(bytes: [u8; 32], key_id: u64, version: u32) -> Self {
        Self {
            inner: Box::new(VolumeKeyInner { bytes, key_id, version }),
        }
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.inner.bytes
    }

    pub fn key_id(&self) -> u64 {
        self.inner.key_id
    }

    pub fn version(&self) -> u32 {
        self.inner.version
    }

    /// Sérialise en VolumeKeyHeader chiffré (prêt pour écriture on-disk).
    pub fn to_header(
        &self,
        master_key: &super::master_key::MasterKey,
        current_epoch: u64,
    ) -> Result<VolumeKeyHeader, FsError> {
        let wrapped = master_key.wrap_volume_key(self.as_bytes())?;
        Ok(VolumeKeyHeader {
            magic:         VOLUME_KEY_MAGIC,
            version:       self.inner.version,
            key_id:        self.inner.key_id,
            wrapped_key:   wrapped,
            created_epoch: current_epoch,
            _reserved:     [0u8; 4],
        })
    }

    /// Désérialise depuis un VolumeKeyHeader on-disk.
    pub fn from_header(
        hdr: &VolumeKeyHeader,
        master_key: &super::master_key::MasterKey,
    ) -> Result<Self, FsError> {
        // RÈGLE 8 : vérifier magic en premier.
        if hdr.magic != VOLUME_KEY_MAGIC {
            return Err(FsError::InvalidMagic);
        }
        let plain = master_key.unwrap_volume_key(&hdr.wrapped_key)?;
        Ok(Self::from_raw(plain, hdr.key_id, hdr.version))
    }

    /// Dérive la clé pour un blob spécifique (ObjectKey).
    pub fn derive_object_key(
        &self,
        blob_id: &crate::fs::exofs::core::BlobId,
    ) -> super::key_derivation::DerivedKey {
        super::key_derivation::KeyDerivation::derive_object_key(self.as_bytes(), blob_id)
    }
}
