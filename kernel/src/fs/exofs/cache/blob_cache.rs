//! blob_cache.rs — Cache de blobs bruts ExoFS (no_std).
//!
//! `BlobCache` : cache LRU/ARC de données blob indexées par `BlobId`.
//! `BLOB_CACHE`  : instance globale statique.
//! Règles : OOM-02, ARITH-02, RECUR-01.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::boxed::Box;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::cache_eviction::{EvictionAlgorithm, EvictionPolicy};
use super::cache_stats::CACHE_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

const BLOB_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// BlobEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans le cache de blobs.
struct BlobEntry {
    /// Données du blob.
    data:          Vec<u8>,
    /// `true` si l'entrée a été modifiée et n'a pas encore été écrite sur disque.
    dirty:         bool,
    /// Ticks d'insertion.
    #[allow(dead_code)]
    inserted_at:   u64,
    /// Ticks du dernier accès.
    last_accessed: u64,
    /// Nombre d'accès.
    access_count:  u64,
}

impl BlobEntry {
    fn new(data: Vec<u8>, now: u64) -> Self {
        Self {
            data,
            dirty:         false,
            inserted_at:   now,
            last_accessed: now,
            access_count:  1,
        }
    }

    fn touch(&mut self, now: u64) {
        self.last_accessed = now;
        self.access_count  = self.access_count.wrapping_add(1);
    }

    fn len(&self) -> u64 { self.data.len() as u64 }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobCacheInner
// ─────────────────────────────────────────────────────────────────────────────

struct BlobCacheInner {
    map:      BTreeMap<BlobId, BlobEntry>,
    eviction: EvictionPolicy,
    used:     u64,
}

impl BlobCacheInner {
    const fn new() -> Self {
        Self {
            map:      BTreeMap::new(),
            eviction: EvictionPolicy::new(EvictionAlgorithm::Arc),
            used:     0,
        }
    }

    fn evict_to_fit(&mut self, needed: u64, max_bytes: u64) -> ExofsResult<()> {
        let mut iters: usize = 0;
        while self.used.saturating_add(needed) > max_bytes {
            let victims = self.eviction.pick_eviction_candidates(4);
            if victims.is_empty() { return Err(ExofsError::NoSpace); }
            for v in &victims {
                if let Some(e) = self.map.remove(v) {
                    let sz = e.len();
                    self.eviction.remove(v);
                    self.used = self.used.saturating_sub(sz);
                    CACHE_STATS.record_eviction(sz);
                }
            }
            iters = iters.wrapping_add(1);
            if iters > 64 { return Err(ExofsError::NoSpace); }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobCache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache de blobs bruts avec éviction et statistiques.
pub struct BlobCache {
    inner:     SpinLock<BlobCacheInner>,
    hits:      AtomicU64,
    misses:    AtomicU64,
    max_bytes: u64,
}

pub static BLOB_CACHE: BlobCache = BlobCache::new_const();

impl BlobCache {
    pub const fn new_const() -> Self {
        Self {
            inner:     SpinLock::new(BlobCacheInner::new()),
            hits:      AtomicU64::new(0),
            misses:    AtomicU64::new(0),
            max_bytes: BLOB_CACHE_MAX_BYTES,
        }
    }

    // ── Lecture ──────────────────────────────────────────────────────────────

    /// Retourne une copie des données du blob, ou `None` si absent.
    pub fn get(&self, id: &BlobId) -> Option<Box<[u8]>> {
        let mut inner = self.inner.lock();
        let now = crate::arch::time::read_ticks();
        if let Some(cloned_data) = inner.map.get_mut(id).map(|e| { e.touch(now); e.data.clone() }) {
            inner.eviction.touch(id);
            let data: Box<[u8]> = cloned_data.into_boxed_slice();
            self.hits.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_hit();
            Some(data)
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_miss();
            None
        }
    }

    /// `true` si le blob est présent en cache.
    pub fn contains(&self, id: &BlobId) -> bool {
        self.inner.lock().map.contains_key(id)
    }

    // ── Écriture ─────────────────────────────────────────────────────────────

    /// Insère ou met à jour un blob dans le cache.
    pub fn insert(&self, id: BlobId, data: Vec<u8>) -> ExofsResult<()> {
        let size = data.len() as u64;
        let now  = crate::arch::time::read_ticks();
        let max  = self.max_bytes;
        let mut inner = self.inner.lock();

        // Si déjà présent, mettre à jour.
        if inner.map.contains_key(&id) {
            let old_size = inner.map[&id].len();
            inner.used = inner.used.saturating_sub(old_size);
            inner.eviction.remove(&id);
            let existing = inner.map.get_mut(&id).unwrap();
            existing.data = data;
            existing.dirty = false;
            existing.touch(now);
            inner.used = inner.used.saturating_add(size);
            inner.eviction.insert(id, size)?;
            CACHE_STATS.record_insert(size);
            return Ok(());
        }

        // OOM-02 : réserver avant insertion.
        inner.evict_to_fit(size, max)?;

        let entry = BlobEntry::new(data, now);
        inner.map.insert(id, entry);
        inner.eviction.insert(id, size)?;
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    /// Invalide (supprime) une entrée du cache.
    pub fn invalidate(&self, id: &BlobId) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.remove(id) {
            let sz = e.len();
            inner.eviction.remove(id);
            inner.used = inner.used.saturating_sub(sz);
            CACHE_STATS.record_invalidation(sz);
        }
    }

    /// Marque une entrée comme dirty (non synchronisée).
    pub fn mark_dirty(&self, id: &BlobId) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(e) => {
                if !e.dirty {
                    e.dirty = true;
                    CACHE_STATS.record_dirty_add(e.len());
                }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    /// Retourne les IDs de toutes les entrées dirty.
    pub fn dirty_ids(&self) -> Vec<BlobId> {
        let inner = self.inner.lock();
        inner.map
            .iter()
            .filter(|(_, e)| e.dirty)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Marque une entrée comme propre (après flush).
    pub fn mark_clean(&self, id: &BlobId) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(e) => {
                if e.dirty {
                    let sz = e.len();
                    e.dirty = false;
                    CACHE_STATS.record_dirty_flush(sz);
                }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    pub fn used_bytes(&self) -> u64 {
        self.inner.lock().used
    }

    pub fn n_entries(&self) -> usize {
        self.inner.lock().map.len()
    }

    /// Retourne la liste de tous les `BlobId` présents dans le cache.
    pub fn list_keys(&self) -> ExofsResult<Vec<BlobId>> {
        let inner = self.inner.lock();
        let mut keys = Vec::new();
        keys.try_reserve(inner.map.len()).map_err(|_| ExofsError::NoMemory)?;
        for k in inner.map.keys() {
            keys.push(*k);
        }
        Ok(keys)
    }

    pub fn hits(&self)   -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn max_bytes(&self) -> u64 { self.max_bytes }

    pub fn hit_ratio_pct(&self) -> u64 {
        let h = self.hits();
        let m = self.misses();
        let t = h.wrapping_add(m);
        if t == 0 { 0 } else { h * 100 / t }
    }

    /// Évince `n` entrées candidates (les plus froides).
    pub fn evict_n(&self, n: usize) -> u64 {
        let mut inner   = self.inner.lock();
        let victims     = inner.eviction.pick_eviction_candidates(n);
        let mut freed   = 0u64;
        for id in &victims {
            if let Some(e) = inner.map.remove(id) {
                let sz = e.len();
                inner.eviction.remove(id);
                inner.used = inner.used.saturating_sub(sz);
                freed = freed.saturating_add(sz);
                CACHE_STATS.record_eviction(sz);
            }
        }
        freed
    }

    /// Vide entièrement le cache.
    pub fn flush_all(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.used = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    #[test] fn test_insert_and_get() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).unwrap();
        assert!(c.get(&blob(1)).is_some());
    }

    #[test] fn test_miss_increments_counter() {
        let c = BlobCache::new_const();
        assert!(c.get(&blob(42)).is_none());
        assert_eq!(c.misses(), 1);
    }

    #[test] fn test_hit_increments_counter() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).unwrap();
        c.get(&blob(1));
        assert_eq!(c.hits(), 1);
    }

    #[test] fn test_invalidate_removes_entry() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).unwrap();
        c.invalidate(&blob(1));
        assert!(c.get(&blob(1)).is_none());
    }

    #[test] fn test_mark_dirty_and_clean() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 32]).unwrap();
        c.mark_dirty(&blob(1)).unwrap();
        assert_eq!(c.dirty_ids().len(), 1);
        c.mark_clean(&blob(1)).unwrap();
        assert_eq!(c.dirty_ids().len(), 0);
    }

    #[test] fn test_mark_dirty_absent_returns_err() {
        let c = BlobCache::new_const();
        assert!(c.mark_dirty(&blob(99)).is_err());
    }

    #[test] fn test_used_bytes_tracks_insertions() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 128]).unwrap();
        assert_eq!(c.used_bytes(), 128);
    }

    #[test] fn test_used_bytes_decreases_on_invalidate() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 128]).unwrap();
        c.invalidate(&blob(1));
        assert_eq!(c.used_bytes(), 0);
    }

    #[test] fn test_contains() {
        let c = BlobCache::new_const();
        assert!(!c.contains(&blob(5)));
        c.insert(blob(5), alloc::vec![0u8; 8]).unwrap();
        assert!(c.contains(&blob(5)));
    }

    #[test] fn test_flush_all() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 64]).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_hit_ratio_pct() {
        let c = BlobCache::new_const();
        c.insert(blob(1), alloc::vec![0u8; 16]).unwrap();
        c.get(&blob(1)); c.get(&blob(1)); c.get(&blob(2));
        assert_eq!(c.hit_ratio_pct(), 66);
    }
}
