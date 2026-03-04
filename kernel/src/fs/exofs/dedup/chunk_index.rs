//! ChunkIndex — index global de tous les chunks connus (no_std).
//!
//! Associe à chaque empreinte blake3 une entrée décrivant le chunk :
//! le BlobId de référence, le nombre de références et la taille.
//!
//! RECUR-01 : aucune récursion.
//! OOM-02   : try_reserve.
//! ARITH-02 : saturating/checked/wrapping.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::chunk_fingerprint::ChunkFingerprint;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const CHUNK_INDEX_MAX_ENTRIES: usize = 1_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// ChunkEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de l'index pour un chunk donné.
#[derive(Clone, Debug)]
pub struct ChunkEntry {
    pub fingerprint: ChunkFingerprint,
    pub blob_id:     BlobId,       // blob de référence (première insertion).
    pub ref_count:   u32,
    pub size:        u32,
}

impl ChunkEntry {
    pub fn new(fingerprint: ChunkFingerprint, blob_id: BlobId, size: u32) -> Self {
        Self { fingerprint, blob_id, ref_count: 1, size }
    }

    /// Incrémente le compteur de références (ARITH-02 checked).
    pub fn inc_ref(&mut self) -> ExofsResult<()> {
        self.ref_count = self.ref_count.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        Ok(())
    }

    /// Décrémente le compteur de références.
    ///
    /// Retourne `true` si le compteur atteint zéro (suppression possible).
    pub fn dec_ref(&mut self) -> bool {
        if self.ref_count > 0 {
            self.ref_count -= 1;
        }
        self.ref_count == 0
    }

    pub fn is_shared(&self) -> bool { self.ref_count > 1 }
    pub fn is_orphan(&self) -> bool { self.ref_count == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkIndexStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques de l'index.
#[derive(Debug, Clone, Copy)]
pub struct ChunkIndexStats {
    pub total_entries:   usize,
    pub shared_chunks:   usize,
    pub orphan_chunks:   usize,
    pub total_ref_count: u64,
    pub total_bytes:     u64,
    pub insertions:      u64,
    pub lookups:         u64,
    pub removals:        u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkIndex
// ─────────────────────────────────────────────────────────────────────────────

/// Index global de chunks, thread-safe (UnsafeCell + spinlock).
pub struct ChunkIndex {
    entries:    UnsafeCell<BTreeMap<[u8; 32], ChunkEntry>>,
    lock:       AtomicU64,
    insertions: AtomicU64,
    lookups:    AtomicU64,
    removals:   AtomicU64,
}

unsafe impl Sync for ChunkIndex {}
unsafe impl Send for ChunkIndex {}

impl ChunkIndex {
    pub const fn new_const() -> Self {
        Self {
            entries:    UnsafeCell::new(BTreeMap::new()),
            lock:       AtomicU64::new(0),
            insertions: AtomicU64::new(0),
            lookups:    AtomicU64::new(0),
            removals:   AtomicU64::new(0),
        }
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }
    fn map(&self) -> &mut BTreeMap<[u8; 32], ChunkEntry> {
        unsafe { &mut *self.entries.get() }
    }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Recherche un chunk par son blake3.
    pub fn lookup(&self, key: &[u8; 32]) -> Option<ChunkEntry> {
        self.acquire();
        let result = self.map().get(key).cloned();
        self.release();
        self.lookups.fetch_add(1, Ordering::Relaxed);
        result
    }

    /// Insère un chunk (ou incrémente son ref_count s'il existe déjà).
    ///
    /// OOM-02 : try_reserve.
    pub fn insert(
        &self,
        fingerprint: ChunkFingerprint,
        blob_id: BlobId,
        size: u32,
    ) -> ExofsResult<bool> { // true = nouveau chunk, false = déjà existant.
        let key = fingerprint.blake3;
        self.acquire();
        let map = self.map();
        let is_new = if let Some(e) = map.get_mut(&key) {
            e.inc_ref()?;
            false
        } else {
            if map.len() >= CHUNK_INDEX_MAX_ENTRIES {
                self.release();
                return Err(ExofsError::NoMemory);
            }
            map.insert(key, ChunkEntry::new(fingerprint, blob_id, size));
            true
        };
        self.release();
        self.insertions.fetch_add(1, Ordering::Relaxed);
        Ok(is_new)
    }

    /// Décrémente le ref_count et retire le chunk si orphelin.
    ///
    /// Retourne `true` si le chunk a été supprimé.
    pub fn decrement_ref(&self, key: &[u8; 32]) -> bool {
        self.acquire();
        let map     = self.map();
        let removed = if let Some(e) = map.get_mut(key) {
            if e.dec_ref() {
                map.remove(key);
                true
            } else {
                false
            }
        } else {
            false
        };
        self.release();
        if removed {
            self.removals.fetch_add(1, Ordering::Relaxed);
        }
        removed
    }

    /// Retire de force un chunk (peu importe le ref_count).
    pub fn force_remove(&self, key: &[u8; 32]) -> bool {
        self.acquire();
        let found = self.map().remove(key).is_some();
        self.release();
        if found { self.removals.fetch_add(1, Ordering::Relaxed); }
        found
    }

    /// Retourne vrai si un chunk est connu.
    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.acquire();
        let found = self.map().contains_key(key);
        self.release();
        found
    }

    /// Retourne le nombre d'entrées.
    pub fn len(&self) -> usize {
        self.acquire();
        let n = self.map().len();
        self.release();
        n
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Vide l'index.
    pub fn clear(&self) {
        self.acquire();
        self.map().clear();
        self.release();
        self.insertions.store(0, Ordering::Relaxed);
        self.lookups.store(0, Ordering::Relaxed);
        self.removals.store(0, Ordering::Relaxed);
    }

    /// Collecte les clés orphelines (ref_count == 0).
    ///
    /// RECUR-01 : boucle for — pas de récursion.
    /// OOM-02   : try_reserve.
    pub fn orphan_keys(&self) -> ExofsResult<Vec<[u8; 32]>> {
        self.acquire();
        let mut keys: Vec<[u8; 32]> = Vec::new();
        for (k, v) in self.map().iter() {
            if v.ref_count == 0 {
                keys.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                keys.push(*k);
            }
        }
        self.release();
        Ok(keys)
    }

    /// Statistiques de l'index.
    ///
    /// RECUR-01 : boucle for.
    /// ARITH-02 : saturating_add.
    pub fn stats(&self) -> ChunkIndexStats {
        self.acquire();
        let mut shared  = 0usize;
        let mut orphan  = 0usize;
        let mut refs    = 0u64;
        let mut bytes   = 0u64;
        for v in self.map().values() {
            if v.is_shared()  { shared  = shared.saturating_add(1); }
            if v.is_orphan()  { orphan  = orphan.saturating_add(1); }
            refs  = refs.saturating_add(v.ref_count as u64);
            bytes = bytes.saturating_add(v.size as u64);
        }
        let total = self.map().len();
        self.release();
        ChunkIndexStats {
            total_entries:   total,
            shared_chunks:   shared,
            orphan_chunks:   orphan,
            total_ref_count: refs,
            total_bytes:     bytes,
            insertions:      self.insertions.load(Ordering::Relaxed),
            lookups:         self.lookups.load(Ordering::Relaxed),
            removals:        self.removals.load(Ordering::Relaxed),
        }
    }

    /// Vérifie l'intégrité grossière (ref_count > 0 pour toutes les entrées).
    pub fn verify_integrity(&self) -> ExofsResult<()> {
        self.acquire();
        let bad = self.map().values().any(|v| v.ref_count == 0);
        self.release();
        if bad { Err(ExofsError::CorruptedStructure) } else { Ok(()) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statique global
// ─────────────────────────────────────────────────────────────────────────────

pub static CHUNK_INDEX: ChunkIndex = ChunkIndex::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};

    fn blob() -> BlobId {
        BlobId::from_raw([0x42u8; 32])
    }

    fn fp(data: &[u8]) -> ChunkFingerprint {
        ChunkFingerprint::compute(data, FingerprintAlgorithm::Double).unwrap()
    }

    #[test] fn test_insert_new_chunk() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"data1");
        let new = idx.insert(f, blob(), 32).unwrap();
        assert!(new);
        assert!(idx.contains(&f.blake3));
    }

    #[test] fn test_insert_existing_increments_ref() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"shared");
        idx.insert(f, blob(), 64).unwrap();
        let new = idx.insert(f, blob(), 64).unwrap();
        assert!(!new);
        let e = idx.lookup(&f.blake3).unwrap();
        assert_eq!(e.ref_count, 2);
    }

    #[test] fn test_decrement_ref_removes_orphan() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"orphan");
        idx.insert(f, blob(), 16).unwrap();
        let removed = idx.decrement_ref(&f.blake3);
        assert!(removed);
        assert!(!idx.contains(&f.blake3));
    }

    #[test] fn test_decrement_ref_keeps_shared() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"keep");
        idx.insert(f, blob(), 16).unwrap();
        idx.insert(f, blob(), 16).unwrap();
        let removed = idx.decrement_ref(&f.blake3);
        assert!(!removed);
        assert_eq!(idx.lookup(&f.blake3).unwrap().ref_count, 1);
    }

    #[test] fn test_stats() {
        let idx = ChunkIndex::new_const();
        let f1  = fp(b"a"); idx.insert(f1, blob(), 10).unwrap();
        let f2  = fp(b"b"); idx.insert(f2, blob(), 20).unwrap();
        idx.insert(f1, blob(), 10).unwrap(); // shared
        let s = idx.stats();
        assert_eq!(s.total_entries, 2);
        assert_eq!(s.shared_chunks, 1);
        assert_eq!(s.total_bytes, 30);
    }

    #[test] fn test_clear() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"clear");
        idx.insert(f, blob(), 8).unwrap();
        idx.clear();
        assert!(idx.is_empty());
    }

    #[test] fn test_orphan_keys() {
        let idx = ChunkIndex::new_const();
        let f   = fp(b"orp");
        idx.insert(f, blob(), 4).unwrap();
        // force ref_count à 0
        idx.acquire();
        idx.map().get_mut(&f.blake3).unwrap().ref_count = 0;
        idx.release();
        let orphans = idx.orphan_keys().unwrap();
        assert_eq!(orphans.len(), 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkIndexSnapshot — instantané de l'index pour audit/debug
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée sérialisable pour un instantané de l'index.
#[derive(Debug, Clone)]
pub struct ChunkIndexSnapshotEntry {
    pub blake3:    [u8; 32],
    pub ref_count: u32,
    pub size:      u32,
}

/// Instantané complet de l'index (pour audit ou migration).
#[derive(Debug, Clone)]
pub struct ChunkIndexSnapshot {
    pub entries: Vec<ChunkIndexSnapshotEntry>,
}

impl ChunkIndex {
    /// Génère un instantané de l'index.
    ///
    /// RECUR-01 : boucle for.
    /// OOM-02   : try_reserve.
    pub fn snapshot(&self) -> ExofsResult<ChunkIndexSnapshot> {
        self.acquire();
        let mut entries: Vec<ChunkIndexSnapshotEntry> = Vec::new();
        for (k, v) in self.map().iter() {
            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(ChunkIndexSnapshotEntry {
                blake3:    *k,
                ref_count: v.ref_count,
                size:      v.size,
            });
        }
        self.release();
        Ok(ChunkIndexSnapshot { entries })
    }
}

#[cfg(test)]
mod tests_snapshot {
    use super::*;
    use super::super::chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};

    fn blob() -> BlobId { BlobId::from_raw([0xBBu8; 32]) }
    fn fp(d: &[u8]) -> ChunkFingerprint {
        ChunkFingerprint::compute(d, FingerprintAlgorithm::Double).unwrap()
    }

    #[test] fn test_snapshot_empty() {
        let idx = ChunkIndex::new_const();
        let s   = idx.snapshot().unwrap();
        assert!(s.entries.is_empty());
    }

    #[test] fn test_snapshot_matches_inserted() {
        let idx = ChunkIndex::new_const();
        idx.insert(fp(b"snap1"), blob(), 8).unwrap();
        idx.insert(fp(b"snap2"), blob(), 16).unwrap();
        let s = idx.snapshot().unwrap();
        assert_eq!(s.entries.len(), 2);
    }
}
