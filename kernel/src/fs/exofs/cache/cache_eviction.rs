//! EvictionPolicy — algorithmes d'éviction du cache ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::BlobId;

/// Algorithme d'éviction.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionAlgorithm {
    Lru   = 0,
    Lfu   = 1,
    Clock = 3,
}

/// Entrée de suivi pour l'éviction.
struct EvictEntry {
    blob_id:   BlobId,
    last_tick: u64,
    freq:      u32,
    size:      u64,
    pinned:    bool,
}

/// Gestionnaire d'éviction générique.
pub struct EvictionPolicy {
    algo:    EvictionAlgorithm,
    entries: BTreeMap<BlobId, EvictEntry>,
    tick:    AtomicU64,
}

impl EvictionPolicy {
    pub fn new(algo: EvictionAlgorithm) -> Self {
        Self {
            algo,
            entries: BTreeMap::new(),
            tick: AtomicU64::new(0),
        }
    }

    pub fn insert(&mut self, blob_id: BlobId, size: u64) {
        let t = self.tick.fetch_add(1, Ordering::Relaxed);
        self.entries.insert(blob_id, EvictEntry {
            blob_id, last_tick: t, freq: 1, size, pinned: false,
        });
    }

    pub fn touch(&mut self, blob_id: &BlobId) {
        let t = self.tick.fetch_add(1, Ordering::Relaxed);
        if let Some(e) = self.entries.get_mut(blob_id) {
            e.last_tick = t;
            e.freq = e.freq.saturating_add(1);
        }
    }

    pub fn remove(&mut self, blob_id: &BlobId) -> Option<u64> {
        self.entries.remove(blob_id).map(|e| e.size)
    }

    pub fn pin(&mut self, blob_id: &BlobId) {
        if let Some(e) = self.entries.get_mut(blob_id) { e.pinned = true; }
    }

    pub fn unpin(&mut self, blob_id: &BlobId) {
        if let Some(e) = self.entries.get_mut(blob_id) { e.pinned = false; }
    }

    /// Sélectionne les N candidats à l'éviction selon l'algorithme.
    pub fn pick_eviction_candidates(&self, n: usize) -> Vec<BlobId> {
        let mut candidates: Vec<(&BlobId, &EvictEntry)> = self.entries
            .iter()
            .filter(|(_, e)| !e.pinned)
            .collect();

        match self.algo {
            EvictionAlgorithm::Lru | EvictionAlgorithm::Clock => {
                candidates.sort_by_key(|(_, e)| e.last_tick);
            }
            EvictionAlgorithm::Lfu => {
                candidates.sort_by_key(|(_, e)| e.freq);
            }
        }

        candidates.iter().take(n).map(|(id, _)| **id).collect()
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn total_bytes(&self) -> u64 { self.entries.values().map(|e| e.size).sum() }
}
