//! BlobSharing — tracking chunks shared across multiple blobs (no_std).
//!
//! Tracks, for each chunk (identified by its blake3), the list of blobs
//! referencing it. Essential for computing dependencies before deleting a blob.
//!
//! RECUR-01: zero recursion.
//! OOM-02  : try_reserve.
//! ARITH-02: saturating / checked / wrapping.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SHARING_MAX_BLOBS_PER_CHUNK: usize = 4096;
pub const SHARING_MAX_ENTRIES: usize = 200_000;

// ─────────────────────────────────────────────────────────────────────────────
// SharedChunkRef
// ─────────────────────────────────────────────────────────────────────────────

/// Set of blobs referencing the same chunk (identified by blake3).
#[derive(Clone, Debug)]
pub struct SharedChunkRef {
    pub chunk_blake3: [u8; 32],
    pub blob_ids: Vec<BlobId>,
}

impl SharedChunkRef {
    pub fn new(chunk_blake3: [u8; 32], first_blob: BlobId) -> ExofsResult<Self> {
        let mut v: Vec<BlobId> = Vec::new();
        v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        v.push(first_blob);
        Ok(Self {
            chunk_blake3,
            blob_ids: v,
        })
    }

    /// Adds a blob referencing this chunk (no duplicates).
    ///
    /// OOM-02: try_reserve.
    pub fn add_blob(&mut self, blob_id: BlobId) -> ExofsResult<()> {
        // Avoid duplicates.
        for existing in &self.blob_ids {
            if existing.as_bytes() == blob_id.as_bytes() {
                return Ok(());
            }
        }
        if self.blob_ids.len() >= SHARING_MAX_BLOBS_PER_CHUNK {
            return Err(ExofsError::NoMemory);
        }
        self.blob_ids
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.blob_ids.push(blob_id);
        Ok(())
    }

    /// Removes a blob.
    pub fn remove_blob(&mut self, blob_id: &BlobId) {
        self.blob_ids.retain(|b| b.as_bytes() != blob_id.as_bytes());
    }

    pub fn is_shared(&self) -> bool {
        self.blob_ids.len() > 1
    }
    pub fn ref_count(&self) -> usize {
        self.blob_ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.blob_ids.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobSharingStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct BlobSharingStats {
    pub total_shared_chunks: usize,
    pub max_sharing_degree: usize,
    pub total_refs: u64,
    pub insertions: u64,
    pub removals: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobSharing
// ─────────────────────────────────────────────────────────────────────────────

/// Chunk <-> blobs sharing registry, thread-safe.
pub struct BlobSharing {
    refs: UnsafeCell<BTreeMap<[u8; 32], SharedChunkRef>>,
    lock: AtomicU64,
    insertions: AtomicU64,
    removals: AtomicU64,
}

unsafe impl Sync for BlobSharing {}
unsafe impl Send for BlobSharing {}

impl BlobSharing {
    pub const fn new_const() -> Self {
        Self {
            refs: UnsafeCell::new(BTreeMap::new()),
            lock: AtomicU64::new(0),
            insertions: AtomicU64::new(0),
            removals: AtomicU64::new(0),
        }
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }
    fn map(&self) -> &mut BTreeMap<[u8; 32], SharedChunkRef> {
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
        unsafe { &mut *self.refs.get() }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Declares that a blob uses a chunk.
    ///
    /// OOM-02: try_reserve in SharedChunkRef.
    pub fn add_ref(&self, chunk_blake3: [u8; 32], blob_id: BlobId) -> ExofsResult<()> {
        self.acquire();
        let map = self.map();
        if let Some(entry) = map.get_mut(&chunk_blake3) {
            if let Err(err) = entry.add_blob(blob_id) {
                self.release();
                return Err(err);
            }
        } else {
            if map.len() >= SHARING_MAX_ENTRIES {
                self.release();
                return Err(ExofsError::NoMemory);
            }
            let entry = match SharedChunkRef::new(chunk_blake3, blob_id) {
                Ok(ent) => ent,
                Err(err) => {
                    self.release();
                    return Err(err);
                }
            };
            map.insert(chunk_blake3, entry);
        }
        self.release();
        self.insertions.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Removes reference of a blob for a chunk.
    /// Removes entry if no blob references the chunk anymore.
    pub fn remove_ref(&self, chunk_blake3: &[u8; 32], blob_id: &BlobId) -> bool {
        self.acquire();
        let map = self.map();
        let removed = if let Some(entry) = map.get_mut(chunk_blake3) {
            entry.remove_blob(blob_id);
            if entry.is_empty() {
                map.remove(chunk_blake3);
                true
            } else {
                false
            }
        } else {
            false
        };
        self.release();
        if removed {
            self.removals.fetch_add(1, Ordering::Relaxed);
        }
        removed
    }

    /// Removes all references of a blob for a list of chunks.
    ///
    /// RECUR-01: for loop.
    pub fn remove_blob_refs(&self, chunk_keys: &[[u8; 32]], blob_id: &BlobId) {
        for key in chunk_keys {
            self.remove_ref(key, blob_id);
        }
    }

    /// Returns blobs sharing a given chunk.
    pub fn blobs_for_chunk(&self, chunk_blake3: &[u8; 32]) -> Option<Vec<BlobId>> {
        self.acquire();
        let result = self.map().get(chunk_blake3).map(|e| e.blob_ids.clone());
        self.release();
        result
    }

    /// Checks whether a chunk is shared across multiple blobs.
    pub fn is_shared(&self, chunk_blake3: &[u8; 32]) -> bool {
        self.acquire();
        let shared = self
            .map()
            .get(chunk_blake3)
            .map(|e| e.is_shared())
            .unwrap_or(false);
        self.release();
        shared
    }

    /// Returns all shared chunks (degree >= 2).
    ///
    /// OOM-02: try_reserve.
    pub fn all_shared_chunks(&self) -> ExofsResult<Vec<[u8; 32]>> {
        self.acquire();
        let mut out: Vec<[u8; 32]> = Vec::new();
        for (k, v) in self.map().iter() {
            if v.is_shared() {
                if out.try_reserve(1).is_err() {
                    self.release();
                    return Err(ExofsError::NoMemory);
                }
                out.push(*k);
            }
        }
        self.release();
        Ok(out)
    }

    pub fn len(&self) -> usize {
        self.acquire();
        let n = self.map().len();
        self.release();
        n
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.acquire();
        self.map().clear();
        self.release();
        self.insertions.store(0, Ordering::Relaxed);
        self.removals.store(0, Ordering::Relaxed);
    }

    /// Sharing statistics.
    ///
    /// ARITH-02: saturating_add.
    pub fn stats(&self) -> BlobSharingStats {
        self.acquire();
        let mut total = 0usize;
        let mut max_deg = 0usize;
        let mut refs = 0u64;
        for v in self.map().values() {
            if v.is_shared() {
                total = total.saturating_add(1);
            }
            if v.ref_count() > max_deg {
                max_deg = v.ref_count();
            }
            refs = refs.saturating_add(v.ref_count() as u64);
        }
        self.release();
        BlobSharingStats {
            total_shared_chunks: total,
            max_sharing_degree: max_deg,
            total_refs: refs,
            insertions: self.insertions.load(Ordering::Relaxed),
            removals: self.removals.load(Ordering::Relaxed),
        }
    }

    /// Verifies integrity (no empty entries).
    pub fn verify_integrity(&self) -> ExofsResult<()> {
        self.acquire();
        let bad = self.map().values().any(|v| v.blob_ids.is_empty());
        self.release();
        if bad {
            Err(ExofsError::CorruptedStructure)
        } else {
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global static
// ─────────────────────────────────────────────────────────────────────────────

pub static BLOB_SHARING: BlobSharing = BlobSharing::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn blob(s: u8) -> BlobId {
        BlobId::from_raw([s; 32])
    }
    fn chunk(s: u8) -> [u8; 32] {
        [s; 32]
    }

    #[test]
    fn test_add_ref_new() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(1), blob(10)).test_unwrap();
        assert!(!bs.is_shared(&chunk(1)));
    }

    #[test]
    fn test_add_ref_shared() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(2), blob(20)).test_unwrap();
        bs.add_ref(chunk(2), blob(21)).test_unwrap();
        assert!(bs.is_shared(&chunk(2)));
    }

    #[test]
    fn test_remove_ref_cleans_up() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(3), blob(30)).test_unwrap();
        bs.remove_ref(&chunk(3), &blob(30));
        assert!(bs.is_empty());
    }

    #[test]
    fn test_blobs_for_chunk() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(4), blob(40)).test_unwrap();
        bs.add_ref(chunk(4), blob(41)).test_unwrap();
        let blobs = bs.blobs_for_chunk(&chunk(4)).test_unwrap();
        assert_eq!(blobs.len(), 2);
    }

    #[test]
    fn test_remove_blob_refs() {
        let bs = BlobSharing::new_const();
        let keys = [chunk(5), chunk(6)];
        bs.add_ref(chunk(5), blob(50)).test_unwrap();
        bs.add_ref(chunk(6), blob(50)).test_unwrap();
        bs.remove_blob_refs(&keys, &blob(50));
        assert!(bs.is_empty());
    }

    #[test]
    fn test_stats() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(7), blob(70)).test_unwrap();
        bs.add_ref(chunk(7), blob(71)).test_unwrap();
        let s = bs.stats();
        assert_eq!(s.total_shared_chunks, 1);
        assert_eq!(s.max_sharing_degree, 2);
    }

    #[test]
    fn test_all_shared_chunks() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(8), blob(80)).test_unwrap();
        bs.add_ref(chunk(8), blob(81)).test_unwrap();
        bs.add_ref(chunk(9), blob(90)).test_unwrap(); // not shared
        let shared = bs.all_shared_chunks().test_unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0], chunk(8));
    }

    #[test]
    fn test_verify_integrity() {
        let bs = BlobSharing::new_const();
        bs.add_ref(chunk(10), blob(100)).test_unwrap();
        assert!(bs.verify_integrity().is_ok());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobDependencyChecker — dependency analysis before deletion
// ─────────────────────────────────────────────────────────────────────────────

/// Result of dependency analysis for blob deletion.
#[derive(Debug, Clone)]
pub struct BlobDeletionAnalysis {
    /// Chunks that would become orphaned after deleting this blob.
    pub chunks_to_delete: Vec<[u8; 32]>,
    /// Chunks that would remain referenced by other blobs.
    pub chunks_to_keep: Vec<[u8; 32]>,
    /// Blobs sharing at least one chunk with the blob to delete.
    pub dependent_blobs: Vec<BlobId>,
}

impl BlobSharing {
    /// Analyzes dependencies before deleting a blob.
    ///
    /// `chunk_keys`: list of chunks of the blob to delete.
    ///
    /// RECUR-01: for loop.
    /// OOM-02  : try_reserve.
    pub fn analyze_deletion(
        &self,
        chunk_keys: &[[u8; 32]],
        blob_id: &BlobId,
    ) -> ExofsResult<BlobDeletionAnalysis> {
        let mut to_delete: Vec<[u8; 32]> = Vec::new();
        let mut to_keep: Vec<[u8; 32]> = Vec::new();
        let mut dep_blobs: Vec<BlobId> = Vec::new();

        for key in chunk_keys {
            self.acquire();
            let entry_clone = self.map().get(key).cloned();
            self.release();

            match entry_clone {
                None => {
                    // Chunk not in sharing registry anymore -> to delete.
                    to_delete.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    to_delete.push(*key);
                }
                Some(entry) => {
                    if entry.ref_count() <= 1 {
                        to_delete.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        to_delete.push(*key);
                    } else {
                        to_keep.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        to_keep.push(*key);
                        // Collect dependent blobs.
                        for b in &entry.blob_ids {
                            if b.as_bytes() != blob_id.as_bytes() {
                                let already = dep_blobs
                                    .iter()
                                    .any(|x: &BlobId| x.as_bytes() == b.as_bytes());
                                if !already {
                                    dep_blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                                    dep_blobs.push(b.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(BlobDeletionAnalysis {
            chunks_to_delete: to_delete,
            chunks_to_keep: to_keep,
            dependent_blobs: dep_blobs,
        })
    }
}

#[cfg(test)]
mod tests_deletion {
    use super::*;

    fn blob(s: u8) -> BlobId {
        BlobId::from_raw([s; 32])
    }
    fn chunk(s: u8) -> [u8; 32] {
        [s; 32]
    }

    #[test]
    fn test_analyze_deletion_sole_owner() {
        let bs = BlobSharing::new_const();
        let k = chunk(0xAA);
        bs.add_ref(k, blob(1)).test_unwrap();
        let analysis = bs.analyze_deletion(&[k], &blob(1)).test_unwrap();
        assert_eq!(analysis.chunks_to_delete.len(), 1);
        assert!(analysis.chunks_to_keep.is_empty());
        assert!(analysis.dependent_blobs.is_empty());
    }

    #[test]
    fn test_analyze_deletion_shared() {
        let bs = BlobSharing::new_const();
        let k = chunk(0xBB);
        bs.add_ref(k, blob(1)).test_unwrap();
        bs.add_ref(k, blob(2)).test_unwrap();
        let analysis = bs.analyze_deletion(&[k], &blob(1)).test_unwrap();
        assert!(analysis.chunks_to_delete.is_empty());
        assert_eq!(analysis.chunks_to_keep.len(), 1);
        assert_eq!(analysis.dependent_blobs.len(), 1);
    }

    #[test]
    fn test_global_sharing_accessible() {
        let s = BLOB_SHARING.stats();
        let _ = s.total_shared_chunks;
    }
}
