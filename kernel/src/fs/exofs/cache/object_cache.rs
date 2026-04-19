//! ObjectCache — cache d'objets structurés (superblock, btree nodes, etc.) (no_std).

//! object_cache.rs — Cache d'objets structurés ExoFS (no_std).
//!
//! `CachedObject` : wrapper autour d'un objet sérialisé (superblock, nœud B-tree, etc.).
//! `ObjectCache` : cache avec comptage de références et éviction.
//! `OBJECT_CACHE` : instance globale statique.
//! Règles : OOM-02, ARITH-02, RECUR-01.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::cache_eviction::{EvictionAlgorithm, EvictionPolicy};
use super::cache_stats::CACHE_STATS;

// ── ObjectKind ────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Superblock     = 0,
    BtreeNode      = 1,
    FreeList       = 2,
    DirectoryBlock = 3,
    InodeTable     = 4,
    Other          = 0xFF,
}

// ── CachedObject ─────────────────────────────────────────────────────────────────────

pub struct CachedObject {
    pub id:          BlobId,
    pub kind:        ObjectKind,
    pub data:        Box<[u8]>,
    pub dirty:       bool,
    pub ref_count:   u32,
    last_accessed:   u64,
    inserted_at:     u64,
    access_count:    u64,
}

impl CachedObject {
    pub fn new(id: BlobId, kind: ObjectKind, data: Box<[u8]>, now: u64) -> Self {
        Self { id, kind, data, dirty: false, ref_count: 0,
               last_accessed: now, inserted_at: now, access_count: 1 }
    }

    pub fn touch(&mut self, now: u64) {
        self.last_accessed = now;
        self.access_count  = self.access_count.wrapping_add(1);
    }

    pub fn size(&self) -> u64 { self.data.len() as u64 }
    pub fn is_pinned(&self) -> bool { self.ref_count > 0 }
}

// ── ObjectCacheInner ──────────────────────────────────────────────────────────────

struct ObjectCacheInner {
    map:      BTreeMap<BlobId, CachedObject>,
    eviction: EvictionPolicy,
    used:     u64,
    max:      u64,
}

impl ObjectCacheInner {
    const fn new(max: u64) -> Self {
        Self { map: BTreeMap::new(), eviction: EvictionPolicy::new(EvictionAlgorithm::Lru),
               used: 0, max }
    }

    fn evict_to_fit(&mut self, needed: u64) -> ExofsResult<()> {
        let mut iters = 0usize;
        while self.used.saturating_add(needed) > self.max {
            let victims = self.eviction.pick_eviction_candidates(4);
            if victims.is_empty() { return Err(ExofsError::NoSpace); }
            for v in &victims {
                if let Some(obj) = self.map.get(v) { if obj.is_pinned() { continue; } }
                if let Some(obj) = self.map.remove(v) {
                    let sz = obj.size();
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

// ── ObjectCache ────────────────────────────────────────────────────────────────────

pub struct ObjectCache {
    inner: SpinLock<ObjectCacheInner>,
}

pub static OBJECT_CACHE: ObjectCache = ObjectCache::new_const();

impl ObjectCache {
    pub const fn new_const() -> Self {
        Self { inner: SpinLock::new(ObjectCacheInner::new(64 * 1024 * 1024)) }
    }

    pub fn insert(&self, obj: CachedObject) -> ExofsResult<()> {
        let id = obj.id; let size = obj.size();
        let mut inner = self.inner.lock();
        inner.evict_to_fit(size)?;
        inner.eviction.insert(id, size)?;
        inner.map.insert(id, obj);
        inner.used = inner.used.saturating_add(size);
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    pub fn get(&self, id: &BlobId) -> Option<Box<[u8]>> {
        let mut inner = self.inner.lock();
        let now = crate::arch::time::read_ticks();
        // L'emprunt de map se termine avant d'accéder à eviction.
        let hit_data = inner.map.get_mut(id).map(|obj| {
            obj.touch(now);
            obj.ref_count = obj.ref_count.saturating_add(1);
            obj.data.clone()
        });
        if hit_data.is_some() {
            inner.eviction.touch(id);
            CACHE_STATS.record_hit();
        } else {
            CACHE_STATS.record_miss();
        }
        hit_data
    }

    pub fn release(&self, id: &BlobId) {
        if let Some(obj) = self.inner.lock().map.get_mut(id) {
            obj.ref_count = obj.ref_count.saturating_sub(1);
        }
    }

    pub fn invalidate(&self, id: &BlobId) {
        let mut inner = self.inner.lock();
        if let Some(obj) = inner.map.remove(id) {
            let sz = obj.size();
            inner.eviction.remove(id);
            inner.used = inner.used.saturating_sub(sz);
            CACHE_STATS.record_invalidation(sz);
        }
    }

    pub fn mark_dirty(&self, id: &BlobId) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(obj) => {
                if !obj.dirty { obj.dirty = true; CACHE_STATS.record_dirty_add(obj.size()); }
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    pub fn update_data(&self, id: &BlobId, new_data: Box<[u8]>) -> ExofsResult<()> {
        let mut inner = self.inner.lock();
        match inner.map.get_mut(id) {
            Some(obj) => {
                let old_sz = obj.size(); let new_sz = new_data.len() as u64;
                obj.data = new_data; obj.dirty = true;
                inner.used = inner.used.saturating_sub(old_sz).saturating_add(new_sz);
                Ok(())
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    pub fn dirty_ids(&self) -> Vec<BlobId> {
        self.inner.lock().map.iter().filter(|(_, o)| o.dirty).map(|(k, _)| *k).collect()
    }

    pub fn flush_all(&self) { let _ = self.drop_all(); }
    pub fn n_entries(&self)  -> usize { self.inner.lock().map.len() }
    pub fn used_bytes(&self) -> u64   { self.inner.lock().used }
    pub fn max_bytes(&self)  -> u64   { self.inner.lock().max }

    pub fn drop_all(&self) -> u64 {
        let mut inner = self.inner.lock();
        let freed = inner.used;
        inner.map.clear();
        inner.used = 0;
        inner.eviction = EvictionPolicy::new(EvictionAlgorithm::Lru);
        freed
    }

    pub fn evict_n(&self, n: usize) -> u64 {
        let mut inner = self.inner.lock();
        let victims = inner.eviction.pick_eviction_candidates(n);
        let mut freed = 0u64;
        for id in &victims {
            if let Some(obj) = inner.map.get(id) { if obj.is_pinned() { continue; } }
            if let Some(obj) = inner.map.remove(id) {
                let sz = obj.size();
                inner.eviction.remove(id);
                inner.used = inner.used.saturating_sub(sz);
                freed = freed.saturating_add(sz);
                CACHE_STATS.record_eviction(sz);
            }
        }
        freed
    }
}
// ── Extensions ObjectCache ───────────────────────────────────────────────────

impl ObjectCache {
    /// Liste les IDs d'un kind donné.
    pub fn ids_by_kind(&self, kind: ObjectKind) -> Vec<BlobId> {
        self.inner.lock().map
            .iter()
            .filter(|(_, o)| o.kind == kind)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Compte les objets par kind.
    pub fn count_by_kind(&self, kind: ObjectKind) -> usize {
        self.inner.lock().map
            .values()
            .filter(|o| o.kind == kind)
            .count()
    }

    /// Vide uniquement les objets dirty (flush partiel).
    pub fn flush_dirty_only(&self) {
        let dirty = self.dirty_ids();
        for id in &dirty { self.invalidate(id); }
    }

    /// Retourne la taille totale des objets pinned.
    pub fn pinned_bytes(&self) -> u64 {
        self.inner.lock().map
            .values()
            .filter(|o| o.is_pinned())
            .map(|o| o.size())
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Force libération d'un objet non pinned par son kind (le plus ancien).
    pub fn evict_oldest_of_kind(&self, kind: ObjectKind) -> Option<u64> {
        let mut inner = self.inner.lock();
        let victim = inner.map
            .iter()
            .filter(|(_, o)| o.kind == kind && !o.is_pinned())
            .min_by_key(|(_, o)| o.inserted_at)
            .map(|(k, _)| *k);
        if let Some(id) = victim {
            if let Some(obj) = inner.map.remove(&id) {
                let sz = obj.size();
                inner.eviction.remove(&id);
                inner.used = inner.used.saturating_sub(sz);
                CACHE_STATS.record_eviction(sz);
                return Some(sz);
            }
        }
        None
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }
    fn mk(b: u8, sz: usize) -> CachedObject {
        CachedObject::new(blob(b), ObjectKind::BtreeNode,
            alloc::vec![0u8; sz].into_boxed_slice(), 0)
    }

    #[test] fn test_insert_get() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        assert!(c.get(&blob(1)).is_some());
    }

    #[test] fn test_miss() {
        let c = ObjectCache::new_const();
        assert!(c.get(&blob(99)).is_none());
    }

    #[test] fn test_invalidate() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.invalidate(&blob(1));
        assert!(c.get(&blob(1)).is_none());
    }

    #[test] fn test_mark_dirty() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.mark_dirty(&blob(1)).unwrap();
        assert_eq!(c.dirty_ids().len(), 1);
    }

    #[test] fn test_used_bytes() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 128)).unwrap();
        assert_eq!(c.used_bytes(), 128);
    }

    #[test] fn test_update_data() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        c.update_data(&blob(1), alloc::vec![0u8; 256].into_boxed_slice()).unwrap();
        assert_eq!(c.used_bytes(), 256);
    }

    #[test] fn test_ref_count_pin() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.get(&blob(1)); // ref_count => 1
        assert!(c.inner.lock().map[&blob(1)].is_pinned());
        c.release(&blob(1));
        assert!(!c.inner.lock().map[&blob(1)].is_pinned());
    }

    #[test] fn test_flush_all() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_evict_n() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        c.insert(mk(2, 64)).unwrap();
        let freed = c.evict_n(1);
        assert!(freed <= 128);
    }

    #[test] fn test_mark_dirty_absent() {
        let c = ObjectCache::new_const();
        assert!(c.mark_dirty(&blob(55)).is_err());
    }

    #[test] fn test_ids_by_kind() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.insert(CachedObject::new(blob(2), ObjectKind::Superblock,
            alloc::vec![0u8;32].into_boxed_slice(), 0)).unwrap();
        assert_eq!(c.ids_by_kind(ObjectKind::BtreeNode).len(), 1);
        assert_eq!(c.count_by_kind(ObjectKind::Superblock), 1);
    }

    #[test] fn test_flush_dirty_only() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 32)).unwrap();
        c.insert(mk(2, 32)).unwrap();
        c.mark_dirty(&blob(1)).unwrap();
        c.flush_dirty_only();
        assert_eq!(c.n_entries(), 1);
    }

    #[test] fn test_pinned_bytes() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        c.get(&blob(1)); // pin
        assert_eq!(c.pinned_bytes(), 64);
    }

    #[test] fn test_evict_oldest_of_kind() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        c.insert(mk(2, 64)).unwrap();
        let freed = c.evict_oldest_of_kind(ObjectKind::BtreeNode);
        assert!(freed.is_some());
    }

    #[test] fn test_flush_all_clears() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 64)).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_used_bytes_zero_after_flush() {
        let c = ObjectCache::new_const();
        c.insert(mk(1, 128)).unwrap();
        c.flush_all();
        assert_eq!(c.used_bytes(), 0);
    }

    #[test] fn test_get_miss() {
        let c = ObjectCache::new_const();
        assert!(c.get(&blob(99)).is_none());
    }

    #[test] fn test_n_entries_after_multiple_inserts() {
        let c = ObjectCache::new_const();
        c.insert(mk(3, 32)).unwrap();
        c.insert(mk(4, 32)).unwrap();
        c.insert(mk(5, 32)).unwrap();
        assert_eq!(c.n_entries(), 3);
    }

    #[test] fn test_pinned_bytes_zero_initially() {
        let c = ObjectCache::new_const();
        c.insert(mk(6, 64)).unwrap();
        // Pas de get() → pas de pin.
        assert_eq!(c.pinned_bytes(), 0);
    }

    #[test] fn test_evict_oldest_of_kind_empty() {
        let c = ObjectCache::new_const();
        let r = c.evict_oldest_of_kind(ObjectKind::BtreeNode);
        assert!(r.is_none());
    }
}
