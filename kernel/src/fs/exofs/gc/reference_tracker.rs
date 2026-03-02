//! Suivi des références entre blobs — utilisé par le GC pour traverser le graphe.
//!
//! Maintient une table blob → {blobs qu'il référence} pour la phase Marking.
//! Construit à partir des métadonnées du BlobStore lors du scan.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::scheduler::sync::spinlock::SpinLock;

/// Table de références sortantes blob → Vec<BlobId>.
pub struct ReferenceTracker {
    inner: SpinLock<BTreeMap<BlobId, Vec<BlobId>>>,
}

impl ReferenceTracker {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(BTreeMap::new()),
        }
    }

    /// Enregistre les références sortantes d'un blob.
    pub fn register_refs(
        &self,
        source: BlobId,
        targets: Vec<BlobId>,
    ) -> Result<(), FsError> {
        let mut map = self.inner.lock();
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(source, targets);
        Ok(())
    }

    /// Retourne les références sortantes d'un blob.
    pub fn get_refs(&self, source: &BlobId) -> Vec<BlobId> {
        let map = self.inner.lock();
        match map.get(source) {
            Some(v) => v.clone(),
            None => Vec::new(),
        }
    }

    /// Ajoute une référence sortante à un blob existant.
    pub fn add_ref(&self, source: &BlobId, target: BlobId) -> Result<(), FsError> {
        let mut map = self.inner.lock();
        let entry = map.entry(*source).or_insert_with(Vec::new);
        entry.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        entry.push(target);
        Ok(())
    }

    /// Supprime toutes les références d'un blob (lors de sa suppression).
    pub fn remove(&self, source: &BlobId) {
        let mut map = self.inner.lock();
        map.remove(source);
    }

    /// Nombre de blobs suivis.
    pub fn tracked_count(&self) -> usize {
        self.inner.lock().len()
    }

    /// Vide complètement la table (entre deux passes GC).
    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

/// Instance globale du tracker de références.
pub static REFERENCE_TRACKER: ReferenceTracker = ReferenceTracker::new();
