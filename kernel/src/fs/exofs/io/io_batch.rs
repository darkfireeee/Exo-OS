//! Batching d'opérations IO ExoFS — regroupe plusieurs petites requêtes en une.
//!
//! Améliore les performances en réduisant la pression sur la queue NVMe.
//! RÈGLE 2  : try_reserve avant tout push.
//! RÈGLE 14 : checked_add pour les offsets.

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::storage::BlobStore;

/// Type d'opération dans un batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoBatchKind {
    Read,
    Write,
    Flush,
}

/// Entrée individuelle d'un batch IO.
pub struct IoBatchEntry {
    pub kind: IoBatchKind,
    pub blob_id: BlobId,
    pub offset: u64,
    /// Données pour les writes (vide pour reads/flush).
    pub data: Vec<u8>,
    /// Résultat (rempli après exécution du batch).
    pub result: Option<Result<usize, FsError>>,
}

impl IoBatchEntry {
    pub fn read(blob_id: BlobId, offset: u64, len: usize) -> Result<Self, FsError> {
        let mut data = Vec::new();
        data.try_reserve(len).map_err(|_| FsError::OutOfMemory)?;
        data.resize(len, 0u8);
        Ok(Self { kind: IoBatchKind::Read, blob_id, offset, data, result: None })
    }

    pub fn write(blob_id: BlobId, offset: u64, data: Vec<u8>) -> Self {
        Self { kind: IoBatchKind::Write, blob_id, offset, data, result: None }
    }

    pub fn flush(blob_id: BlobId) -> Self {
        Self { kind: IoBatchKind::Flush, blob_id, offset: 0, data: Vec::new(), result: None }
    }
}

/// Batch d'opérations IO à exécuter séquentiellement.
pub struct IoBatch {
    entries: Vec<IoBatchEntry>,
}

impl IoBatch {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Ajoute une entrée au batch.
    pub fn push(&mut self, entry: IoBatchEntry) -> Result<(), FsError> {
        self.entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Exécute toutes les opérations du batch.
    /// Les erreurs individuelles sont stockées dans `entry.result` — pas d'abort.
    pub fn execute(&mut self, store: &BlobStore) -> Result<(), FsError> {
        for entry in &mut self.entries {
            entry.result = Some(match entry.kind {
                IoBatchKind::Read => {
                    store.read_blob_range(&entry.blob_id, entry.offset, &mut entry.data)
                }
                IoBatchKind::Write => {
                    store.write_blob_at(&entry.blob_id, entry.offset, &entry.data)
                }
                IoBatchKind::Flush => {
                    store.flush_blob(&entry.blob_id).map(|_| 0)
                }
            });
        }
        Ok(())
    }

    /// Nombre d'entrées dans le batch.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Itère sur les résultats.
    pub fn results(&self) -> impl Iterator<Item = Option<&Result<usize, FsError>>> {
        self.entries.iter().map(|e| e.result.as_ref())
    }
}
