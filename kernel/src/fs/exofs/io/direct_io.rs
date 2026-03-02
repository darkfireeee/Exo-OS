//! Direct IO ExoFS — lecture/écriture sans cache, alignée sur le secteur NVMe.
//!
//! Bypass complet du cache blob. Utilisé pour les snapshots et la restauration.
//! RÈGLE 7  : 3 barrières NVMe dans le commit Epoch.
//! RÈGLE 14 : checked_add pour TOUS calculs d'offset disque.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::storage::BlobStore;

/// Alignement secteur NVMe (512 bytes).
pub const NVME_SECTOR_SIZE: u64 = 512;

/// Vérifie que `offset` et `len` sont alignés secteur.
fn check_alignment(offset: u64, len: usize) -> Result<(), FsError> {
    if offset % NVME_SECTOR_SIZE != 0 || len as u64 % NVME_SECTOR_SIZE != 0 {
        Err(FsError::AlignmentError)
    } else {
        Ok(())
    }
}

/// Couche Direct IO — accès disque sans intermediate cache.
pub struct DirectIo<'store> {
    store: &'store BlobStore,
}

impl<'store> DirectIo<'store> {
    pub fn new(store: &'store BlobStore) -> Self {
        Self { store }
    }

    /// Lecture directe secteur-alignée d'un blob à `offset` dans `buf`.
    /// `offset` et `buf.len()` doivent être des multiples de 512.
    pub fn read_aligned(
        &self,
        id: &BlobId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        check_alignment(offset, buf.len())?;
        self.store.read_blob_range(id, offset, buf)
    }

    /// Écriture directe secteur-alignée.
    /// RÈGLE 7 : le caller doit émettre les barrières NVMe autour de ce call
    /// si imbriqué dans un commit Epoch.
    pub fn write_aligned(
        &self,
        id: &BlobId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, FsError> {
        check_alignment(offset, data.len())?;
        self.store.write_blob_at(id, offset, data)
    }

    /// Flush forcé vers le média NVMe (flush queue + barrière).
    pub fn flush_sync(&self, id: &BlobId) -> Result<(), FsError> {
        self.store.flush_blob(id)?;
        // Barrière NVMe (requête FUA ou FLUSH).
        self.store.nvme_flush_barrier()
    }
}
