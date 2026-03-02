//! ChunkCache — cache LRU de chunks récemment accédés pour la déduplication (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::chunk_fingerprint::ChunkFingerprint;

/// Taille maximale du cache (nombre de chunks).
const CHUNK_CACHE_CAPACITY: usize = 4096;

struct CacheEntry {
    fp:       ChunkFingerprint,
    blob_id:  BlobId,
    lru_tick: u64,
}

pub struct ChunkCache {
    entries:  SpinLock<BTreeMap<[u8; 32], CacheEntry>>,
    hits:     AtomicU64,
    misses:   AtomicU64,
    evictions: AtomicU64,
    tick:     AtomicU64,
}

pub static CHUNK_CACHE: ChunkCache = ChunkCache::new_const();

impl ChunkCache {
    pub const fn new_const() -> Self {
        Self {
            entries:   SpinLock::new(BTreeMap::new()),
            hits:      AtomicU64::new(0),
            misses:    AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            tick:      AtomicU64::new(0),
        }
    }

    fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed)
    }

    pub fn lookup(&self, fp: &ChunkFingerprint) -> Option<BlobId> {
        let mut entries = self.entries.lock();
        if let Some(e) = entries.get_mut(&fp.blake3) {
            if e.fp.matches(fp) {
                e.lru_tick = self.next_tick();
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(e.blob_id);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    pub fn insert(&self, fp: ChunkFingerprint, blob_id: BlobId) -> Result<(), FsError> {
        let tick = self.next_tick();
        let mut entries = self.entries.lock();

        // Éviction LRU si au-delà de la capacité.
        if entries.len() >= CHUNK_CACHE_CAPACITY {
            let lru_key = entries
                .iter()
                .min_by_key(|(_, e)| e.lru_tick)
                .map(|(k, _)| *k);
            if let Some(k) = lru_key {
                entries.remove(&k);
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        entries.insert(fp.blake3, CacheEntry { fp, blob_id, lru_tick: tick });
        Ok(())
    }

    pub fn invalidate(&self, fp: &ChunkFingerprint) {
        self.entries.lock().remove(&fp.blake3);
    }

    pub fn size(&self) -> usize { self.entries.lock().len() }
    pub fn hits(&self) -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self) -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn evictions(&self) -> u64 { self.evictions.load(Ordering::Relaxed) }
}
