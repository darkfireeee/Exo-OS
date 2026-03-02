//! ObjectCache — cache d'objets structurés (superblock, btree nodes, etc.) (no_std).

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::cache_stats::CACHE_STATS;

/// Identifiant d'objet dans le cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectCacheId {
    pub kind:  u8,    // 0=SuperBlock, 1=BTreeNode, 2=InodeTable, …
    pub index: u64,
}

/// Données opaques d'un objet mis en cache.
pub struct CachedObject {
    pub data:    Box<[u8]>,
    pub dirty:   bool,
    pub version: u64,
}

pub static OBJECT_CACHE: ObjectCache = ObjectCache::new_const();
const OBJECT_CACHE_MAX: usize = 4096;

pub struct ObjectCache {
    map:  SpinLock<BTreeMap<ObjectCacheId, CachedObject>>,
    hits: AtomicU64,
    miss: AtomicU64,
}

impl ObjectCache {
    pub const fn new_const() -> Self {
        Self {
            map:  SpinLock::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            miss: AtomicU64::new(0),
        }
    }

    pub fn get(&self, id: &ObjectCacheId) -> Option<alloc::vec::Vec<u8>> {
        let map = self.map.lock();
        if let Some(e) = map.get(id) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_hit();
            return Some(e.data.to_vec());
        }
        self.miss.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_miss();
        None
    }

    pub fn insert(
        &self,
        id: ObjectCacheId,
        data: alloc::vec::Vec<u8>,
        version: u64,
    ) -> Result<(), FsError> {
        let mut map = self.map.lock();
        if map.len() >= OBJECT_CACHE_MAX {
            if let Some(k) = map.keys().next().copied() { map.remove(&k); }
        }
        let size = data.len() as u64;
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(id, CachedObject {
            data: data.into_boxed_slice(),
            dirty: false,
            version,
        });
        CACHE_STATS.record_insert(size);
        Ok(())
    }

    pub fn mark_dirty(&self, id: &ObjectCacheId) {
        let mut map = self.map.lock();
        if let Some(e) = map.get_mut(id) { e.dirty = true; }
    }

    pub fn invalidate(&self, id: &ObjectCacheId) {
        let mut map = self.map.lock();
        if let Some(e) = map.remove(id) {
            CACHE_STATS.record_eviction(e.data.len() as u64);
        }
    }

    pub fn dirty_ids(&self) -> alloc::vec::Vec<ObjectCacheId> {
        let map = self.map.lock();
        map.iter().filter(|(_, e)| e.dirty).map(|(k, _)| *k).collect()
    }

    pub fn len(&self) -> usize { self.map.lock().len() }
    pub fn hits(&self) -> u64  { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.miss.load(Ordering::Relaxed) }
}
