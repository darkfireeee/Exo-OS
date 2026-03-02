//! block_cache.rs — Cache de blocs bruts pour le storage ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

const BLOCK_CACHE_MAX: usize = 1024;

/// Entrée de cache de bloc.
struct BlockEntry {
    data:    Box<[u8]>,
    lru_tick: u64,
    dirty:   bool,
}

/// Cache LRU de blocs lus depuis le disque.
pub struct BlockCache {
    entries:  SpinLock<BTreeMap<u64, BlockEntry>>,   // LBA → données.
    tick:     AtomicU64,
    hits:     AtomicU64,
    misses:   AtomicU64,
}

impl BlockCache {
    pub const fn new_const() -> Self {
        Self {
            entries: SpinLock::new(BTreeMap::new()),
            tick:    AtomicU64::new(0),
            hits:    AtomicU64::new(0),
            misses:  AtomicU64::new(0),
        }
    }

    pub fn get(&self, lba: u64, block_size: usize) -> Option<alloc::vec::Vec<u8>> {
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock();
        if let Some(e) = entries.get_mut(&lba) {
            e.lru_tick = tick;
            self.hits.fetch_add(1, Ordering::Relaxed);
            let mut v = alloc::vec::Vec::new();
            let _ = v.try_reserve(e.data.len());
            v.extend_from_slice(&e.data);
            Some(v)
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn insert(&self, lba: u64, data: &[u8]) -> Result<(), FsError> {
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock();

        // Éviction si plein.
        if entries.len() >= BLOCK_CACHE_MAX {
            let &evict_lba = entries.iter()
                .min_by_key(|(_, e)| e.lru_tick)
                .map(|(k, _)| k)
                .unwrap_or(&0);
            entries.remove(&evict_lba);
        }

        let mut buf = alloc::vec::Vec::new();
        buf.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
        buf.extend_from_slice(data);

        entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        entries.insert(lba, BlockEntry { data: buf.into_boxed_slice(), lru_tick: tick, dirty: false });
        Ok(())
    }

    pub fn invalidate(&self, lba: u64) {
        self.entries.lock().remove(&lba);
    }

    pub fn hits(&self)   -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn len(&self)    -> usize { self.entries.lock().len() }
}

pub static BLOCK_CACHE: BlockCache = BlockCache::new_const();
