//! BlobSharing — gestion du partage de blobs dédupliqués entre inodes (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};

/// Liste des inodes partageant un blob.
#[derive(Clone, Debug)]
pub struct SharingEntry {
    pub blob_id: BlobId,
    pub inodes:  Vec<u64>,  // Inode IDs partageant ce blob.
}

pub struct BlobSharing {
    map:            SpinLock<BTreeMap<BlobId, SharingEntry>>,
    shared_blobs:   AtomicU64,
}

pub static BLOB_SHARING: BlobSharing = BlobSharing::new_const();

impl BlobSharing {
    pub const fn new_const() -> Self {
        Self {
            map:          SpinLock::new(BTreeMap::new()),
            shared_blobs: AtomicU64::new(0),
        }
    }

    /// Enregistre qu'un inode référence un blob.
    pub fn add_ref(&self, blob_id: BlobId, inode_id: u64) -> Result<(), FsError> {
        let mut map = self.map.lock();
        if let Some(e) = map.get_mut(&blob_id) {
            if !e.inodes.contains(&inode_id) {
                e.inodes.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                e.inodes.push(inode_id);
            }
            return Ok(());
        }
        let mut inodes = Vec::new();
        inodes.try_reserve(4).map_err(|_| FsError::OutOfMemory)?;
        inodes.push(inode_id);
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(blob_id, SharingEntry { blob_id, inodes });
        self.shared_blobs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retire la référence d'un inode vers un blob.
    /// Retourne `true` si plus aucun inode ne référence le blob.
    pub fn remove_ref(&self, blob_id: &BlobId, inode_id: u64) -> bool {
        let mut map = self.map.lock();
        if let Some(e) = map.get_mut(blob_id) {
            e.inodes.retain(|&id| id != inode_id);
            if e.inodes.is_empty() {
                map.remove(blob_id);
                self.shared_blobs.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    pub fn get_inodes(&self, blob_id: &BlobId) -> Option<Vec<u64>> {
        self.map.lock().get(blob_id).map(|e| e.inodes.clone())
    }

    pub fn ref_count(&self, blob_id: &BlobId) -> u32 {
        self.map.lock().get(blob_id).map(|e| e.inodes.len() as u32).unwrap_or(0)
    }

    pub fn shared_blobs(&self) -> u64 { self.shared_blobs.load(Ordering::Relaxed) }
}
