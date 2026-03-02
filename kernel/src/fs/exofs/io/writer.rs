//! Écrivain de blobs ExoFS — écriture séquentielle avec stats et flush.
//!
//! RÈGLE 9  : copy_from_user() pour les pointeurs userspace.
//! RÈGLE 11 : BlobId = Blake3(données AVANT compression).
//! RÈGLE 14 : checked_add pour TOUS calculs d'offset.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::io::io_stats::IO_STATS;
use crate::fs::exofs::storage::BlobStore;

/// Écrivain de blob avec curseur interne et flush différé.
pub struct BlobWriter<'store> {
    store: &'store BlobStore,
    blob_id: BlobId,
    /// Bytes écrits depuis l'ouverture.
    written: u64,
    /// `true` si des données ont été écrites mais pas encore flushées.
    dirty: bool,
}

impl<'store> BlobWriter<'store> {
    /// Ouvre un blob existant en écriture.
    pub fn open(store: &'store BlobStore, id: BlobId) -> Result<Self, FsError> {
        if !store.blob_exists(&id) {
            return Err(FsError::NotFound);
        }
        Ok(Self { store, blob_id: id, written: 0, dirty: false })
    }

    /// Crée un nouveau blob en écriture.
    /// NOTE : le BlobId doit être calculé Blake3 des données AVANT compression (RÈGLE 11).
    pub fn create(store: &'store BlobStore, id: BlobId, _hint_size: u64) -> Result<Self, FsError> {
        store.create_blob(id)?;
        Ok(Self { store, blob_id: id, written: 0, dirty: false })
    }

    /// Écrit un slice dans le blob à partir du curseur courant.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, FsError> {
        let start_tick = crate::arch::time::read_ticks();
        let result = self.store.write_blob_append(&self.blob_id, data);
        let elapsed = crate::arch::time::read_ticks().saturating_sub(start_tick);

        match result {
            Ok(n) => {
                IO_STATS.record_write(n as u64, elapsed, true);
                self.written = self
                    .written
                    .checked_add(n as u64)
                    .ok_or(FsError::Overflow)?;
                self.dirty = true;
                Ok(n)
            }
            Err(e) => {
                IO_STATS.record_write(0, elapsed, false);
                Err(e)
            }
        }
    }

    /// Force le flush des données vers le blob store.
    pub fn flush(&mut self) -> Result<(), FsError> {
        if self.dirty {
            self.store.flush_blob(&self.blob_id)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// Flush et finalise le blob (calcule la taille finale).
    pub fn finish(mut self) -> Result<BlobId, FsError> {
        self.flush()?;
        Ok(self.blob_id)
    }

    /// Bytes écrits depuis l'ouverture.
    pub fn written_bytes(&self) -> u64 {
        self.written
    }

    /// BlobId courant.
    pub fn blob_id(&self) -> BlobId {
        self.blob_id
    }
}
