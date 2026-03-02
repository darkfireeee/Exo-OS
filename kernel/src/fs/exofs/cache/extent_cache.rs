//! ExtentCache — cache d'extents (plages d'octets) pour les blobs ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::cache_stats::CACHE_STATS;

/// Clé d'une entrée d'extent : (BlobId, offset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtentKey {
    pub blob_id: BlobId,
    pub offset:  u64,
}

/// Entrée d'extent en cache.
pub struct ExtentEntry {
    pub data:   Box<[u8]>,
    pub length: u32,
    pub dirty:  bool,
}

pub static EXTENT_CACHE: ExtentCache = ExtentCache::new_const();
const EXTENT_CACHE_MAX: usize = 8192;

pub struct ExtentCache {
    map:   SpinLock<BTreeMap<ExtentKey, ExtentEntry>>,
    hits:  AtomicU64,
    miss:  AtomicU64,
}

impl ExtentCache {
    pub const fn new_const() -> Self {
        Self {
            map:  SpinLock::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            miss: AtomicU64::new(0),
        }
    }

    pub fn get(&self, blob_id: &BlobId, offset: u64) -> Option<Box<[u8]>> {
        let map = self.map.lock();
        let key = ExtentKey { blob_id: *blob_id, offset };
        if let Some(e) = map.get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_hit();
            // SAFETY: Box<[u8]> implémente Clone via into_vec().to_boxed_slice().
            return Some(e.data.iter().cloned().collect::<alloc::vec::Vec<_>>().into_boxed_slice());
        }
        self.miss.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_miss();
        None
    }

    pub fn insert(
        &self,
        blob_id: BlobId,
        offset: u64,
        data: alloc::vec::Vec<u8>,
    ) -> Result<(), FsError> {
        let mut map = self.map.lock();
        if map.len() >= EXTENT_CACHE_MAX {
            // Éviction simple : retire la première entrée (le plus ancien).
            if let Some(k) = map.keys().next().copied() {
                if let Some(e) = map.remove(&k) {
                    CACHE_STATS.record_eviction(e.length as u64);
                }
            }
        }
        let length = data.len() as u32;
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        map.insert(
            ExtentKey { blob_id, offset },
            ExtentEntry { data: data.into_boxed_slice(), length, dirty: false },
        );
        CACHE_STATS.record_insert(length as u64);
        Ok(())
    }

    pub fn invalidate(&self, blob_id: &BlobId) {
        let mut map = self.map.lock();
        map.retain(|k, _| &k.blob_id != blob_id);
    }

    pub fn hits(&self) -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.miss.load(Ordering::Relaxed) }
    pub fn len(&self) -> usize { self.map.lock().len() }
}
