//! MetadataCache — cache des métadonnées d'inodes/entrées de répertoire (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::cache_stats::CACHE_STATS;

const META_CACHE_MAX: usize = 32768;

/// Métadonnées d'inode (snapshot en cache).
#[derive(Clone, Debug)]
pub struct InodeMeta {
    pub inode_id:   u64,
    pub size:       u64,
    pub flags:      u32,
    pub n_blobs:    u32,
    pub cached_tick: u64,
}

pub static METADATA_CACHE: MetadataCache = MetadataCache::new_const();

pub struct MetadataCache {
    map:   SpinLock<BTreeMap<u64, InodeMeta>>,  // Clé = inode_id.
    hits:  AtomicU64,
    miss:  AtomicU64,
}

impl MetadataCache {
    pub const fn new_const() -> Self {
        Self {
            map:  SpinLock::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            miss: AtomicU64::new(0),
        }
    }

    pub fn get(&self, inode_id: u64) -> Option<InodeMeta> {
        let map = self.map.lock();
        let r = map.get(&inode_id).cloned();
        if r.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_hit();
        } else {
            self.miss.fetch_add(1, Ordering::Relaxed);
            CACHE_STATS.record_miss();
        }
        r
    }

    pub fn insert(&self, meta: InodeMeta) -> Result<(), FsError> {
        let mut map = self.map.lock();
        let tick = crate::arch::time::read_ticks();
        if map.len() >= META_CACHE_MAX {
            // Éviction : retire l'entrée la plus ancienne (tick le plus bas).
            let oldest = map.iter().min_by_key(|(_, v)| v.cached_tick).map(|(k, _)| *k);
            if let Some(k) = oldest { map.remove(&k); }
        }
        let inode_id = meta.inode_id;
        map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        let mut m = meta;
        m.cached_tick = tick;
        map.insert(inode_id, m);
        CACHE_STATS.record_insert(core::mem::size_of::<InodeMeta>() as u64);
        Ok(())
    }

    pub fn invalidate(&self, inode_id: u64) {
        self.map.lock().remove(&inode_id);
    }

    pub fn invalidate_batch(&self, inode_ids: &[u64]) {
        let mut map = self.map.lock();
        for id in inode_ids { map.remove(id); }
    }

    pub fn size(&self) -> usize { self.map.lock().len() }
    pub fn hits(&self) -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.miss.load(Ordering::Relaxed) }
}
