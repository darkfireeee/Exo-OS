//! IO bufférisé ExoFS — lecture avec buffer interne pour les petites requêtes.
//!
//! RÈGLE 10 : buffer per-CPU pour PATH_MAX — jamais [u8;4096] sur stack kernel.
//! RÈGLE 14 : checked_add pour les offsets.

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::io::reader::BlobReader;
use crate::fs::exofs::storage::BlobStore;

/// Taille du buffer interne (4 KiB = page size).
const BUFFER_SIZE: usize = 4096;

/// Lecteur bufférisé pour améliorer les performances des petites lectures.
///
/// Le buffer est alloué sur le tas (heap) — JAMAIS sur la stack kernel (RÈGLE 10).
pub struct BufferedReader<'store> {
    inner: BlobReader<'store>,
    /// Buffer alloué sur le heap.
    buf: Vec<u8>,
    /// Position du buffer dans le blob.
    buf_offset: u64,
    /// Bytes valides dans le buffer.
    buf_valid: usize,
}

impl<'store> BufferedReader<'store> {
    /// Crée un lecteur bufférisé.
    pub fn new(store: &'store BlobStore, id: BlobId) -> Result<Self, FsError> {
        let inner = BlobReader::open(store, id)?;
        let mut buf = Vec::new();
        buf.try_reserve(BUFFER_SIZE).map_err(|_| FsError::OutOfMemory)?;
        buf.resize(BUFFER_SIZE, 0u8);
        Ok(Self { inner, buf, buf_offset: 0, buf_valid: 0 })
    }

    /// Lit jusqu'à `out.len()` bytes depuis le curseur courant.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, FsError> {
        let cursor = self.inner.pos();
        let buf_end = self.buf_offset.checked_add(self.buf_valid as u64)
            .ok_or(FsError::Overflow)?;

        // Vérifier si les données sont déjà dans le buffer.
        if cursor >= self.buf_offset && cursor < buf_end {
            let buf_pos = (cursor - self.buf_offset) as usize;
            let available = self.buf_valid - buf_pos;
            let to_copy = out.len().min(available);
            out[..to_copy].copy_from_slice(&self.buf[buf_pos..buf_pos + to_copy]);
            self.inner.seek(cursor.checked_add(to_copy as u64).ok_or(FsError::Overflow)?)?;
            return Ok(to_copy);
        }

        // Remplir le buffer depuis le blob.
        self.buf_offset = cursor;
        self.buf_valid = self.inner.read(&mut self.buf)?;
        if self.buf_valid == 0 {
            return Ok(0); // EOF
        }

        let to_copy = out.len().min(self.buf_valid);
        out[..to_copy].copy_from_slice(&self.buf[..to_copy]);
        self.inner.seek(cursor.checked_add(to_copy as u64).ok_or(FsError::Overflow)?)?;
        Ok(to_copy)
    }

    /// Invalide le buffer (après un write concurrent).
    pub fn invalidate(&mut self) {
        self.buf_valid = 0;
    }

    /// Déplace le curseur et invalide si hors du buffer courant.
    pub fn seek(&mut self, offset: u64) -> Result<(), FsError> {
        let buf_end = self.buf_offset.checked_add(self.buf_valid as u64)
            .ok_or(FsError::Overflow)?;
        if offset < self.buf_offset || offset >= buf_end {
            self.invalidate();
        }
        self.inner.seek(offset)
    }
}
