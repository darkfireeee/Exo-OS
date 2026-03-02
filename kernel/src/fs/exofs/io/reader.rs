//! Lecteur de blobs ExoFS — lecture séquentielle ou aléatoire avec stats.
//!
//! RÈGLE 9  : copy_from_user() obligatoire pour pointeurs userspace.
//! RÈGLE 14 : checked_add pour TOUS calculs d'offset.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::io::io_stats::IO_STATS;
use crate::fs::exofs::storage::BlobStore;

/// Lecteur de blob positionnant un curseur interne.
pub struct BlobReader<'store> {
    store: &'store BlobStore,
    blob_id: BlobId,
    /// Curseur de lecture courant (bytes depuis le début du blob).
    cursor: u64,
    /// Taille totale du blob (cache).
    total_len: u64,
}

impl<'store> BlobReader<'store> {
    /// Ouvre un blob en lecture. Vérifie l'existence et cache la taille.
    pub fn open(store: &'store BlobStore, id: BlobId) -> Result<Self, FsError> {
        let total_len = store.blob_size(&id)?;
        Ok(Self { store, blob_id: id, cursor: 0, total_len })
    }

    /// Lit jusqu'à `buf.len()` bytes depuis le curseur courant.
    /// Retourne le nombre de bytes effectivement lus.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        if self.cursor >= self.total_len {
            return Ok(0); // EOF
        }
        let remaining = self.total_len.saturating_sub(self.cursor);
        let to_read = (buf.len() as u64).min(remaining) as usize;

        let start_tick = crate::arch::time::read_ticks();
        let result = self.store.read_blob_range(
            &self.blob_id,
            self.cursor,
            &mut buf[..to_read],
        );
        let elapsed = crate::arch::time::read_ticks().saturating_sub(start_tick);

        match result {
            Ok(n) => {
                IO_STATS.record_read(n as u64, elapsed, true);
                self.cursor = self.cursor.checked_add(n as u64).ok_or(FsError::Overflow)?;
                Ok(n)
            }
            Err(e) => {
                IO_STATS.record_read(0, elapsed, false);
                Err(e)
            }
        }
    }

    /// Lecture à une position absolue sans modifier le curseur.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        if offset >= self.total_len {
            return Ok(0);
        }
        let remaining = self.total_len.saturating_sub(offset);
        let to_read = (buf.len() as u64).min(remaining) as usize;
        self.store.read_blob_range(&self.blob_id, offset, &mut buf[..to_read])
    }

    /// Déplace le curseur.
    pub fn seek(&mut self, offset: u64) -> Result<(), FsError> {
        if offset > self.total_len {
            return Err(FsError::InvalidOffset);
        }
        self.cursor = offset;
        Ok(())
    }

    /// Taille totale du blob.
    pub fn len(&self) -> u64 {
        self.total_len
    }

    /// Curseur courant.
    pub fn pos(&self) -> u64 {
        self.cursor
    }

    /// `true` si le curseur est en fin de blob.
    pub fn eof(&self) -> bool {
        self.cursor >= self.total_len
    }
}
