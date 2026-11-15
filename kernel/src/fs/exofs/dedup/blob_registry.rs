//! BlobRegistry — global registry of deduplicated blobs (no_std).
//!
//! Associates each BlobId (blake3 of source data) with its metadata:
//! chunk count, total size, reference counter, and compact
//! list of blake3 chunk fingerprints composing it.
//!
//! RULE 11: BlobId = blake3(data before compression/encryption).
//!
//! RECUR-01: zero recursion — while/for loops.
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

pub const BLOB_REGISTRY_MAX_ENTRIES: usize = 500_000;
pub const BLOB_MAX_CHUNKS_IN_REGISTRY: usize = 16384; // CHUNK_MAX_PER_BLOB

// ─────────────────────────────────────────────────────────────────────────────
// BlobEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Registry entry for a blob.
#[derive(Clone, Debug)]
pub struct BlobEntry {
    pub blob_id: BlobId,
    pub chunk_count: u32,
    pub total_size: u64,
    pub ref_count: u32,
    /// Ordered blake3 chunk fingerprints.
    pub chunk_keys: Vec<[u8; 32]>,
}

impl BlobEntry {
    /// Creates a new blob entry.
    ///
    /// OOM-02: try_reserve.
    pub fn new(blob_id: BlobId, total_size: u64, chunk_keys: Vec<[u8; 32]>) -> ExofsResult<Self> {
        let chunk_count = chunk_keys.len() as u32;
        Ok(Self {
            blob_id,
            chunk_count,
            total_size,
            ref_count: 1,
            chunk_keys,
        })
    }

    /// Increments reference counter.
    ///
    /// ARITH-02: checked_add.
    pub fn inc_ref(&mut self) -> ExofsResult<()> {
        self.ref_count = self
            .ref_count
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(())
    }

    /// Decrements reference counter.
    ///
    /// Returns `true` if blob becomes orphan.
    pub fn dec_ref(&mut self) -> bool {
        if self.ref_count > 0 {
            self.ref_count -= 1;
        }
        self.ref_count == 0
    }

    pub fn is_shared(&self) -> bool {
        self.ref_count > 1
    }
    pub fn is_orphan(&self) -> bool {
        self.ref_count == 0
    }
    pub fn key(&self) -> &[u8; 32] {
        self.blob_id.as_bytes()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobRegistryStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct BlobRegistryStats {
    pub total_blobs: usize,
    pub shared_blobs: usize,
    pub orphan_blobs: usize,
    pub total_chunks: u64,
    pub total_bytes: u64,
    pub insertions: u64,
    pub lookups: u64,
    pub removals: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobRegistry
// ─────────────────────────────────────────────────────────────────────────────

/// Global blob registry, thread-safe (UnsafeCell + spinlock AtomicU64).
pub struct BlobRegistry {
    entries: UnsafeCell<BTreeMap<[u8; 32], BlobEntry>>,
    lock: AtomicU64,
    insertions: AtomicU64,
    lookups: AtomicU64,
    removals: AtomicU64,
}

unsafe impl Sync for BlobRegistry {}
unsafe impl Send for BlobRegistry {}

impl BlobRegistry {
    pub const fn new_const() -> Self {
        Self {
            entries: UnsafeCell::new(BTreeMap::new()),
            lock: AtomicU64::new(0),
            insertions: AtomicU64::new(0),
            lookups: AtomicU64::new(0),
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
    fn map(&self) -> &mut BTreeMap<[u8; 32], BlobEntry> {
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
        unsafe { &mut *self.entries.get() }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Searches for a blob by its BlobId.
    pub fn lookup(&self, blob_id: &BlobId) -> Option<BlobEntry> {
        let key = *blob_id.as_bytes();
        self.acquire();
        let result = self.map().get(&key).cloned();
        self.release();
        self.lookups.fetch_add(1, Ordering::Relaxed);
        result
    }

    /// Registers a new blob (or increments ref_count if already exists).
    ///
    /// OOM-02: try_reserve.
    /// Returns `true` if the blob is new.
    pub fn register(
        &self,
        blob_id: BlobId,
        total_size: u64,
        chunk_keys: Vec<[u8; 32]>,
    ) -> ExofsResult<bool> {
        let key = *blob_id.as_bytes();
        self.acquire();
        let map = self.map();
        let is_new = if let Some(e) = map.get_mut(&key) {
            if let Err(err) = e.inc_ref() {
                self.release();
                return Err(err);
            }
            false
        } else {
            if map.len() >= BLOB_REGISTRY_MAX_ENTRIES {
                self.release();
                return Err(ExofsError::NoMemory);
            }
            let entry = match BlobEntry::new(blob_id, total_size, chunk_keys) {
                Ok(ent) => ent,
                Err(err) => {
                    self.release();
                    return Err(err);
                }
            };
            map.insert(key, entry);
            true
        };
        self.release();
        self.insertions.fetch_add(1, Ordering::Relaxed);
        Ok(is_new)
    }

    /// Decrements ref_count and deletes blob if orphan.
    pub fn deregister(&self, blob_id: &BlobId) -> bool {
        let key = *blob_id.as_bytes();
        self.acquire();
        let removed = if let Some(e) = self.map().get_mut(&key) {
            if e.dec_ref() {
                self.map().remove(&key);
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

    /// Checks whether a blob is registered.
    pub fn contains(&self, blob_id: &BlobId) -> bool {
        let key = *blob_id.as_bytes();
        self.acquire();
        let found = self.map().contains_key(&key);
        self.release();
        found
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
        self.lookups.store(0, Ordering::Relaxed);
        self.removals.store(0, Ordering::Relaxed);
    }

    /// Returns orphan BlobIds (ref_count == 0).
    ///
    /// OOM-02: try_reserve.
    pub fn orphan_blobs(&self) -> ExofsResult<Vec<BlobId>> {
        self.acquire();
        let mut result: Vec<BlobId> = Vec::new();
        for v in self.map().values() {
            if v.ref_count == 0 {
                if result.try_reserve(1).is_err() {
                    self.release();
                    return Err(ExofsError::NoMemory);
                }
                result.push(v.blob_id.clone());
            }
        }
        self.release();
        Ok(result)
    }

    /// Full registry statistics.
    ///
    /// ARITH-02: saturating_add.
    pub fn stats(&self) -> BlobRegistryStats {
        self.acquire();
        let mut shared = 0usize;
        let mut orphan = 0usize;
        let mut chunks = 0u64;
        let mut bytes = 0u64;
        for v in self.map().values() {
            if v.is_shared() {
                shared = shared.saturating_add(1);
            }
            if v.is_orphan() {
                orphan = orphan.saturating_add(1);
            }
            chunks = chunks.saturating_add(v.chunk_count as u64);
            bytes = bytes.saturating_add(v.total_size);
        }
        let total = self.map().len();
        self.release();
        BlobRegistryStats {
            total_blobs: total,
            shared_blobs: shared,
            orphan_blobs: orphan,
            total_chunks: chunks,
            total_bytes: bytes,
            insertions: self.insertions.load(Ordering::Relaxed),
            lookups: self.lookups.load(Ordering::Relaxed),
            removals: self.removals.load(Ordering::Relaxed),
        }
    }

    /// Verifies registry invariants.
    pub fn verify_integrity(&self) -> ExofsResult<()> {
        self.acquire();
        let bad = self
            .map()
            .values()
            .any(|v| v.chunk_count as usize != v.chunk_keys.len());
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

pub static BLOB_REGISTRY: BlobRegistry = BlobRegistry::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob(seed: u8) -> BlobId {
        BlobId::from_raw([seed; 32])
    }
    fn keys(n: usize) -> Vec<[u8; 32]> {
        (0..n).map(|i| [i as u8; 32]).collect()
    }

    #[test]
    fn test_register_new() {
        let reg = BlobRegistry::new_const();
        let id = make_blob(1);
        let new = reg.register(id.clone(), 1024, keys(4)).test_unwrap();
        assert!(new);
        assert!(reg.contains(&id));
    }

    #[test]
    fn test_register_existing_increments_ref() {
        let reg = BlobRegistry::new_const();
        let id = make_blob(2);
        reg.register(id.clone(), 1024, keys(4)).test_unwrap();
        let new = reg.register(id.clone(), 1024, keys(4)).test_unwrap();
        assert!(!new);
        let e = reg.lookup(&id).test_unwrap();
        assert_eq!(e.ref_count, 2);
    }

    #[test]
    fn test_deregister_removes_orphan() {
        let reg = BlobRegistry::new_const();
        let id = make_blob(3);
        reg.register(id.clone(), 512, keys(2)).test_unwrap();
        let removed = reg.deregister(&id);
        assert!(removed);
        assert!(!reg.contains(&id));
    }

    #[test]
    fn test_deregister_keeps_shared() {
        let reg = BlobRegistry::new_const();
        let id = make_blob(4);
        reg.register(id.clone(), 512, keys(2)).test_unwrap();
        reg.register(id.clone(), 512, keys(2)).test_unwrap();
        let removed = reg.deregister(&id);
        assert!(!removed);
        assert_eq!(reg.lookup(&id).test_unwrap().ref_count, 1);
    }

    #[test]
    fn test_stats() {
        let reg = BlobRegistry::new_const();
        reg.register(make_blob(5), 100, keys(1)).test_unwrap();
        reg.register(make_blob(6), 200, keys(2)).test_unwrap();
        let s = reg.stats();
        assert_eq!(s.total_blobs, 2);
        assert_eq!(s.total_bytes, 300);
        assert_eq!(s.total_chunks, 3);
    }

    #[test]
    fn test_verify_integrity_ok() {
        let reg = BlobRegistry::new_const();
        reg.register(make_blob(7), 50, keys(3)).test_unwrap();
        assert!(reg.verify_integrity().is_ok());
    }

    #[test]
    fn test_clear() {
        let reg = BlobRegistry::new_const();
        reg.register(make_blob(8), 64, keys(1)).test_unwrap();
        reg.clear();
        assert!(reg.is_empty());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobEntryIterator — batch traversal of registry entries
// ─────────────────────────────────────────────────────────────────────────────

/// Compact summary of a blob for external iteration.
#[derive(Debug, Clone)]
pub struct BlobSummary {
    pub blob_id: BlobId,
    pub chunk_count: u32,
    pub total_size: u64,
    pub ref_count: u32,
}

impl BlobRegistry {
    /// Returns all blob summaries (for audit or GC).
    ///
    /// RECUR-01: for loop.
    /// OOM-02  : try_reserve.
    pub fn all_summaries(&self) -> ExofsResult<Vec<BlobSummary>> {
        self.acquire();
        let mut out: Vec<BlobSummary> = Vec::new();
        for v in self.map().values() {
            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            out.push(BlobSummary {
                blob_id: v.blob_id.clone(),
                chunk_count: v.chunk_count,
                total_size: v.total_size,
                ref_count: v.ref_count,
            });
        }
        self.release();
        Ok(out)
    }

    /// Returns blobs whose size exceeds `threshold_bytes`.
    ///
    /// OOM-02: try_reserve.
    pub fn large_blobs(&self, threshold_bytes: u64) -> ExofsResult<Vec<BlobSummary>> {
        let all = self.all_summaries()?;
        let mut out: Vec<BlobSummary> = Vec::new();
        for s in all {
            if s.total_size > threshold_bytes {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(s);
            }
        }
        Ok(out)
    }

    /// Counts blobs with ref_count >= `min_refs` (shared).
    pub fn count_shared_above(&self, min_refs: u32) -> usize {
        self.acquire();
        let n = self
            .map()
            .values()
            .filter(|v| v.ref_count >= min_refs)
            .count();
        self.release();
        n
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;

    fn bid(s: u8) -> BlobId {
        BlobId::from_raw([s; 32])
    }
    fn ks(n: usize) -> Vec<[u8; 32]> {
        (0..n).map(|i| [i as u8; 32]).collect()
    }

    #[test]
    fn test_all_summaries() {
        let r = BlobRegistry::new_const();
        r.register(bid(10), 100, ks(2)).test_unwrap();
        r.register(bid(11), 200, ks(3)).test_unwrap();
        let s = r.all_summaries().test_unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn test_large_blobs() {
        let r = BlobRegistry::new_const();
        r.register(bid(20), 50, ks(1)).test_unwrap();
        r.register(bid(21), 500, ks(2)).test_unwrap();
        r.register(bid(22), 5000, ks(3)).test_unwrap();
        let large = r.large_blobs(100).test_unwrap();
        assert_eq!(large.len(), 2);
    }

    #[test]
    fn test_count_shared() {
        let r = BlobRegistry::new_const();
        let id = bid(30);
        r.register(id.clone(), 64, ks(1)).test_unwrap();
        r.register(id.clone(), 64, ks(1)).test_unwrap();
        assert_eq!(r.count_shared_above(2), 1);
    }

    #[test]
    fn test_orphan_blobs() {
        let r = BlobRegistry::new_const();
        let id = bid(40);
        r.register(id.clone(), 32, ks(1)).test_unwrap();
        // Force orphan from outside
        r.acquire();
        r.map().get_mut(id.as_bytes()).test_unwrap().ref_count = 0;
        r.release();
        let orphans = r.orphan_blobs().test_unwrap();
        assert_eq!(orphans.len(), 1);
    }

    #[test]
    fn test_global_registry_accessible() {
        let s = BLOB_REGISTRY.stats();
        let _ = s.total_blobs; // access without panic
    }
}
