//! BlobRegistry — registre des blobs dédupliqués dans ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};

/// Entrée du registre pour un blob connu.
#[derive(Clone, Debug)]
pub struct RegistryEntry {
    pub blob_id:    BlobId,
    pub ref_count:  u32,
    pub size:       u64,
    pub n_chunks:   u32,
    pub first_seen: u64,  // Timestamp en ticks.
}

pub static BLOB_REGISTRY: BlobRegistry = BlobRegistry::new_const();

pub struct BlobRegistry {
    map:        SpinLock<BTreeMap<BlobId, RegistryEntry>>,
    total_blobs: AtomicU64,
    dedup_saved: AtomicU64,  // Bytes économisés par déduplication.
}

impl BlobRegistry {
    pub const fn new_const() -> Self {
        Self {
            map:         SpinLock::new(BTreeMap::new()),
            total_blobs:  AtomicU64::new(0),
            dedup_saved:  AtomicU64::new(0),
        }
    }

    /// Enregistre un nouveau blob ou incrémente son refcount.
    /// Retourne `true` si c'est un doublon (dédup hit).
    pub fn register(
        &self,
        blob_id: BlobId,
        size: u64,
        n_chunks: u32,
    ) -> Result<bool, FsError> {
        let tick = crate::arch::time::read_ticks();
        let mut map = self.map.lock();
        if let Some(e) = map.get_mut(&blob_id) {
            e.ref_count = e.ref_count.saturating_add(1);
            self.dedup_saved.fetch_add(size, Ordering::Relaxed);
            return Ok(true); // Dédup hit.
        }
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(blob_id, RegistryEntry {
            blob_id, ref_count: 1, size, n_chunks, first_seen: tick,
        });
        self.total_blobs.fetch_add(1, Ordering::Relaxed);
        Ok(false) // Nouveau blob.
    }

    /// Décrémente le refcount. Retourne `true` si le blob peut être supprimé.
    pub fn dec_ref(&self, blob_id: &BlobId) -> bool {
        let mut map = self.map.lock();
        if let Some(e) = map.get_mut(blob_id) {
            if e.ref_count <= 1 {
                map.remove(blob_id);
                self.total_blobs.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
            e.ref_count -= 1;
        }
        false
    }

    pub fn get(&self, blob_id: &BlobId) -> Option<RegistryEntry> {
        self.map.lock().get(blob_id).cloned()
    }

    pub fn total_blobs(&self) -> u64 { self.total_blobs.load(Ordering::Relaxed) }
    pub fn dedup_saved_bytes(&self) -> u64 { self.dedup_saved.load(Ordering::Relaxed) }
}
