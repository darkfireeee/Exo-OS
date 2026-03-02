//! BlobCache — cache LRU/ARC de blobs ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::cache_eviction::EvictionPolicy;
use super::cache_eviction::EvictionAlgorithm;
use super::cache_stats::CACHE_STATS;

pub static BLOB_CACHE: BlobCache = BlobCache::new_const();

const BLOB_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB.

struct BlobEntry {
    data:    alloc::boxed::Box<[u8]>,
    dirty:   bool,
}

struct BlobCacheInner {
    map:      BTreeMap<BlobId, BlobEntry>,
    eviction: EvictionPolicy,
    used:     u64,
}

pub struct BlobCache {
    inner:    SpinLock<BlobCacheInner>,
    hits:     AtomicU64,
    misses:   AtomicU64,
    max_bytes: u64,
}

impl BlobCache {
    pub const fn new_const() -> Self {
        Self {
            inner: SpinLock::new(BlobCacheInner {
                map:      BTreeMap::new(),
                eviction: EvictionPolicy::new(EvictionAlgorithm::Lru),
                used:     0,
            }),
            hits:      AtomicU64::new(0),
            misses:    AtomicU64::new(0),
            max_bytes: BLOB_CACHE_MAX_BYTES,
        }
    }

    /// Lit un blob depuis le cache. Retourne `None` si absent.
    pub fn get(&self, blob_id: &BlobId) -> Option<alloc::boxed::Box<[u8]>> {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.get(blob_id) {
            let data = e.data.clone();
            inner.eviction.touch(blob_id);
            self.hits.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_hit();
            return Some(data);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_miss();
        None
    }

    /// Insère un blob dans le cache.
    pub fn insert(&self, blob_id: BlobId, data: Vec<u8>) -> Result<(), FsError> {
        let size = data.len() as u64;
        self.evict_if_needed(size)?;
        let boxed: alloc::boxed::Box<[u8]> = data.into_boxed_slice();
        let mut inner = self.inner.lock();
        inner.map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        inner.map.insert(blob_id, BlobEntry { data: boxed, dirty: false });
        inner.eviction.insert(blob_id, size);
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    /// Invalide un blob du cache.
    pub fn invalidate(&self, blob_id: &BlobId) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.remove(blob_id) {
            let sz = e.data.len() as u64;
            inner.used = inner.used.saturating_sub(sz);
            inner.eviction.remove(blob_id);
            CACHE_STATS.record_eviction(sz);
        }
    }

    /// Marque un blob comme dirty (modifié, writeback requis).
    pub fn mark_dirty(&self, blob_id: &BlobId) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.get_mut(blob_id) {
            e.dirty = true;
        }
    }

    fn evict_if_needed(&self, needed: u64) -> Result<(), FsError> {
        let mut inner = self.inner.lock();
        if inner.used.saturating_add(needed) <= self.max_bytes { return Ok(()); }
        let candidates = inner.eviction.pick_eviction_candidates(16);
        for bid in candidates {
            if inner.used.saturating_add(needed) <= self.max_bytes { break; }
            if let Some(e) = inner.map.remove(&bid) {
                let sz = e.data.len() as u64;
                inner.used = inner.used.saturating_sub(sz);
                inner.eviction.remove(&bid);
                CACHE_STATS.record_eviction(sz);
            }
        }
        Ok(())
    }

    pub fn used_bytes(&self) -> u64 { self.inner.lock().used }
    pub fn hits(&self) -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.misses.load(Ordering::Relaxed) }
}
