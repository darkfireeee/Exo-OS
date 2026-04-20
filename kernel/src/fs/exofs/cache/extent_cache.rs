//! ExtentCache — cache d'extents (plages d'octets) pour les blobs ExoFS (no_std).

//! extent_cache.rs — Cache d'extents ExoFS (no_std).
//!
//! Un extent = plage d'octets `[offset, offset+len)` dans un blob.
//! `EXTENT_CACHE` : instance globale statique.
//! Règles : OOM-02, ARITH-02, RECUR-01.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::cache_eviction::{EvictionAlgorithm, EvictionPolicy};
use super::cache_stats::CACHE_STATS;

// ── ExtentKey ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtentKey {
    pub blob_id: BlobId,
    pub offset:  u64,
}

// ── ExtentEntry ─────────────────────────────────────────────────────────────────

pub struct ExtentEntry {
    pub data:          Box<[u8]>,
    pub len:           u32,
    pub dirty:         bool,
    pub last_accessed: u64,
    pub access_count:  u64,
}

impl ExtentEntry {
    fn new(data: Box<[u8]>, now: u64) -> Self {
        let len = data.len() as u32;
        Self { data, len, dirty: false, last_accessed: now, access_count: 1 }
    }

    fn touch(&mut self, now: u64) {
        self.last_accessed = now;
        self.access_count = self.access_count.wrapping_add(1);
    }

    pub fn size(&self) -> u64 { self.len as u64 }
}

// ── ExtentCacheKey (clef éviction = BlobId XOR offset) ────────────────────────

/// Clef EvictionPolicy : hash simple de (BlobId, offset).
fn eviction_id(blob_id: &BlobId, offset: u64) -> BlobId {
    let mut id = *blob_id;
    let ob = offset.to_le_bytes();
    for i in 0..8 { id.0[i] ^= ob[i]; }
    id
}

// ── ExtentCacheInner ─────────────────────────────────────────────────────────────

struct ExtentCacheInner {
    map:      BTreeMap<ExtentKey, ExtentEntry>,
    eviction: EvictionPolicy,
    used:     u64,
    max:      u64,
}

impl ExtentCacheInner {
    const fn new(max: u64) -> Self {
        Self { map: BTreeMap::new(), eviction: EvictionPolicy::new(EvictionAlgorithm::Lru),
               used: 0, max }
    }

    fn evict_to_fit(&mut self, needed: u64) -> ExofsResult<()> {
        let mut iters = 0usize;
        while self.used.saturating_add(needed) > self.max {
            let victims = self.eviction.pick_eviction_candidates(4);
            if victims.is_empty() { return Err(ExofsError::NoSpace); }
            for eid in &victims {
                // Retrouver la clé ExtentKey à partir du eid (cherche par valeur).
                let found = self.map.iter()
                    .find(|(k, _)| eviction_id(&k.blob_id, k.offset) == *eid)
                    .map(|(k, _)| *k);
                if let Some(key) = found {
                    if let Some(e) = self.map.remove(&key) {
                        let sz = e.size();
                        self.eviction.remove(eid);
                        self.used = self.used.saturating_sub(sz);
                        CACHE_STATS.record_eviction(sz);
                    }
                }
            }
            iters = iters.wrapping_add(1);
            if iters > 64 { return Err(ExofsError::NoSpace); }
        }
        Ok(())
    }
}

// ── ExtentCache ─────────────────────────────────────────────────────────────────

pub struct ExtentCache {
    inner: SpinLock<ExtentCacheInner>,
}

pub static EXTENT_CACHE: ExtentCache = ExtentCache::new_const();

impl ExtentCache {
    pub const fn new_const() -> Self {
        Self { inner: SpinLock::new(ExtentCacheInner::new(128 * 1024 * 1024)) }
    }

    pub fn get(&self, blob_id: &BlobId, offset: u64) -> Option<Box<[u8]>> {
        let mut inner = self.inner.lock();
        let now = crate::arch::time::read_ticks();
        let key = ExtentKey { blob_id: *blob_id, offset };
        let eid = eviction_id(blob_id, offset);
        // L'emprunt de map se termine avant d'accéder à eviction.
        let hit_data = inner.map.get_mut(&key).map(|e| {
            e.touch(now);
            e.data.iter().cloned().collect::<Vec<_>>().into_boxed_slice()
        });
        if hit_data.is_some() {
            inner.eviction.touch(&eid);
            CACHE_STATS.record_hit();
        } else {
            CACHE_STATS.record_miss();
        }
        hit_data
    }

    /// Cherche tous les extents couvrant `[offset, offset+len)`.
    pub fn get_covering(&self, blob_id: &BlobId, offset: u64, len: u64) -> Vec<(u64, Box<[u8]>)> {
        let inner = self.inner.lock();
        let end = offset.saturating_add(len);
        inner.map.range(
            ExtentKey { blob_id: *blob_id, offset } ..
            ExtentKey { blob_id: *blob_id, offset: end }
        )
        .filter(|(k, _)| k.blob_id == *blob_id)
        .map(|(k, e)| (k.offset, e.data.iter().cloned().collect::<Vec<_>>().into_boxed_slice()))
        .collect()
    }

    pub fn insert(&self, blob_id: BlobId, offset: u64, data: Vec<u8>) -> ExofsResult<()> {
        let now  = crate::arch::time::read_ticks();
        let size = data.len() as u64;
        let eid  = eviction_id(&blob_id, offset);
        let key  = ExtentKey { blob_id, offset };
        let mut inner = self.inner.lock();
        inner.evict_to_fit(size)?;
        inner.eviction.insert(eid, size)?;
        inner.map.insert(key, ExtentEntry::new(data.into_boxed_slice(), now));
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    pub fn invalidate_blob(&self, blob_id: &BlobId) {
        let mut inner = self.inner.lock();
        let to_remove: Vec<ExtentKey> = inner.map
            .keys()
            .filter(|k| k.blob_id == *blob_id)
            .cloned()
            .collect();
        for k in to_remove {
            if let Some(e) = inner.map.remove(&k) {
                let sz = e.size();
                inner.eviction.remove(&eviction_id(&k.blob_id, k.offset));
                inner.used = inner.used.saturating_sub(sz);
                CACHE_STATS.record_invalidation(sz);
            }
        }
    }

    pub fn mark_dirty(&self, blob_id: &BlobId, offset: u64) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        let key = ExtentKey { blob_id: *blob_id, offset };
        match inner.map.get_mut(&key) {
            Some(e) => {
                if !e.dirty { e.dirty = true; CACHE_STATS.record_dirty_add(e.size()); }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    pub fn n_entries(&self)  -> usize { self.inner.lock().map.len() }
    pub fn used_bytes(&self) -> u64   { self.inner.lock().used }
    pub fn max_bytes(&self)  -> u64   { self.inner.lock().max }

    pub fn flush_all(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.used = 0;
    }
}

// ── Extensions ───────────────────────────────────────────────────────────────

impl ExtentCache {
    /// Marque tous les extents d'un blob comme dirty.
    pub fn mark_all_dirty(&self, blob_id: &crate::fs::exofs::core::BlobId) {
        let mut inner = self.inner.lock();
        for (k, e) in inner.map.iter_mut() {
            if k.blob_id == *blob_id && !e.dirty {
                e.dirty = true;
                CACHE_STATS.record_dirty_add(e.size());
            }
        }
    }

    /// Retourne les clés (offset) de tous les extents dirty d'un blob.
    pub fn dirty_offsets(&self, blob_id: &crate::fs::exofs::core::BlobId) -> Vec<u64> {
        self.inner.lock().map
            .iter()
            .filter(|(k, e)| k.blob_id == *blob_id && e.dirty)
            .map(|(k, _)| k.offset)
            .collect()
    }

    /// Invalide les extents dans la plage `[offset, offset+len)`.
    pub fn invalidate_range(
        &self,
        blob_id: &crate::fs::exofs::core::BlobId,
        offset: u64,
        len: u64,
    ) {
        let end = offset.saturating_add(len);
        let mut inner = self.inner.lock();
        let to_remove: Vec<ExtentKey> = inner.map
            .keys()
            .filter(|k| k.blob_id == *blob_id && k.offset >= offset && k.offset < end)
            .cloned()
            .collect();
        for k in to_remove {
            if let Some(e) = inner.map.remove(&k) {
                let sz = e.size();
                inner.eviction.remove(&eviction_id(&k.blob_id, k.offset));
                inner.used = inner.used.saturating_sub(sz);
                CACHE_STATS.record_invalidation(sz);
            }
        }
    }

    /// Évince `n` extents (les plus froids).
    pub fn evict_n(&self, n: usize) -> u64 {
        let mut inner = self.inner.lock();
        let victims = inner.eviction.pick_eviction_candidates(n);
        let mut freed = 0u64;
        for eid in &victims {
            let found = inner.map
                .iter()
                .find(|(k, _)| eviction_id(&k.blob_id, k.offset) == *eid)
                .map(|(k, _)| *k);
            if let Some(key) = found {
                if let Some(e) = inner.map.remove(&key) {
                    let sz = e.size();
                    inner.eviction.remove(eid);
                    inner.used = inner.used.saturating_sub(sz);
                    freed = freed.saturating_add(sz);
                    CACHE_STATS.record_eviction(sz);
                }
            }
        }
        freed
    }

    /// Nombre total d'octets dirty.
    pub fn dirty_bytes(&self) -> u64 {
        self.inner.lock().map
            .values()
            .filter(|e| e.dirty)
            .map(|e| e.size())
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }
}


// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    #[test] fn test_insert_get() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 128]).unwrap();
        assert!(c.get(&blob(1), 0).is_some());
    }

    #[test] fn test_miss() {
        let c = ExtentCache::new_const();
        assert!(c.get(&blob(1), 0).is_none());
    }

    #[test] fn test_used_bytes() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 256]).unwrap();
        assert_eq!(c.used_bytes(), 256);
    }

    #[test] fn test_invalidate_blob_removes_extents() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0,    alloc::vec![0u8; 128]).unwrap();
        c.insert(blob(1), 4096, alloc::vec![0u8; 128]).unwrap();
        c.insert(blob(2), 0,    alloc::vec![0u8; 128]).unwrap();
        c.invalidate_blob(&blob(1));
        assert!(c.get(&blob(1), 0).is_none());
        assert!(c.get(&blob(2), 0).is_some());
    }

    #[test] fn test_mark_dirty() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 64]).unwrap();
        c.mark_dirty(&blob(1), 0).unwrap();
    }

    #[test] fn test_mark_dirty_absent() {
        let c = ExtentCache::new_const();
        assert!(c.mark_dirty(&blob(99), 0).is_err());
    }

    #[test] fn test_get_covering_range() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0,    alloc::vec![1u8; 64]).unwrap();
        c.insert(blob(1), 64,   alloc::vec![2u8; 64]).unwrap();
        c.insert(blob(1), 4096, alloc::vec![3u8; 64]).unwrap();
        let covering = c.get_covering(&blob(1), 0, 128);
        assert_eq!(covering.len(), 2);
    }

    #[test] fn test_flush_all() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 32]).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_n_entries() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0,    alloc::vec![0u8; 16]).unwrap();
        c.insert(blob(1), 1024, alloc::vec![0u8; 16]).unwrap();
        assert_eq!(c.n_entries(), 2);
    }

    #[test] fn test_mark_all_dirty() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0,    alloc::vec![0u8; 64]).unwrap();
        c.insert(blob(1), 4096, alloc::vec![0u8; 64]).unwrap();
        c.mark_all_dirty(&blob(1));
        let offs = c.dirty_offsets(&blob(1));
        assert_eq!(offs.len(), 2);
    }

    #[test] fn test_dirty_offsets_empty_when_clean() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 32]).unwrap();
        assert!(c.dirty_offsets(&blob(1)).is_empty());
    }

    #[test] fn test_invalidate_range() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0,    alloc::vec![0u8; 64]).unwrap();
        c.insert(blob(1), 512,  alloc::vec![0u8; 64]).unwrap();
        c.insert(blob(1), 8192, alloc::vec![0u8; 64]).unwrap();
        c.invalidate_range(&blob(1), 0, 1024);
        assert!(c.get(&blob(1), 0).is_none());
        assert!(c.get(&blob(1), 512).is_none());
        assert!(c.get(&blob(1), 8192).is_some()); // hors plage
    }

    #[test] fn test_dirty_bytes() {
        let c = ExtentCache::new_const();
        c.insert(blob(1), 0, alloc::vec![0u8; 256]).unwrap();
        c.mark_dirty(&blob(1), 0).unwrap();
        assert_eq!(c.dirty_bytes(), 256);
    }

    #[test] fn test_n_entries_after_insert() {
        let c = ExtentCache::new_const();
        c.insert(blob(2), 0,   alloc::vec![0u8; 64]).unwrap();
        c.insert(blob(2), 512, alloc::vec![0u8; 64]).unwrap();
        assert_eq!(c.n_entries(), 2);
    }

    #[test] fn test_used_bytes_after_insert() {
        let c = ExtentCache::new_const();
        c.insert(blob(3), 0, alloc::vec![0u8; 128]).unwrap();
        assert_eq!(c.used_bytes(), 128);
    }

    #[test] fn test_flush_all_clears() {
        let c = ExtentCache::new_const();
        c.insert(blob(4), 0, alloc::vec![0u8; 64]).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_get_miss() {
        let c = ExtentCache::new_const();
        assert!(c.get(&blob(99), 0).is_none());
    }

    #[test] fn test_evict_n() {
        let c = ExtentCache::new_const();
        c.insert(blob(5), 0, alloc::vec![0u8; 32]).unwrap();
        c.insert(blob(5), 64, alloc::vec![0u8; 32]).unwrap();
        let freed = c.evict_n(1);
        assert!(freed >= 32);
    }
}
