//! ChunkCache — Cache LRU de chunks dédupliqués (no_std).
//!
//! Stocke les données des chunks récemment accédés pour éviter
//! de relire le stockage. Utilise un compteur d'accès pour LRU approximatif.
//!
//! RECUR-01 : aucune récursion — boucles while/for.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : saturating / checked / wrapping.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::chunk_fingerprint::{ChunkFingerprint, blake3_hash};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const CHUNK_CACHE_DEFAULT_CAPACITY: usize = 1024;
pub const CHUNK_CACHE_MAX_CAPACITY:     usize = 65536;
/// Quota max en octets pour éviter de saturer la RAM (64 MiB).
pub const CHUNK_CACHE_MAX_BYTES:        u64   = 64 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// ChunkCacheEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée du cache pour un chunk.
#[derive(Clone)]
pub struct ChunkCacheEntry {
    pub fingerprint: ChunkFingerprint,
    pub data:        Vec<u8>,
    pub accesses:    u64,
    pub size:        u32,
}

impl ChunkCacheEntry {
    /// Crée une entrée de cache depuis un fingerprint et des données.
    ///
    /// OOM-02 : try_reserve.
    pub fn new(fingerprint: ChunkFingerprint, data: &[u8]) -> ExofsResult<Self> {
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        let size = data.len() as u32;
        Ok(Self { fingerprint, data: v, accesses: 0, size })
    }

    /// Incrémente le compteur d'accès (ARITH-02 saturating).
    pub fn touch(&mut self) {
        self.accesses = self.accesses.saturating_add(1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkCacheStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du cache.
#[derive(Debug, Clone, Copy)]
pub struct ChunkCacheStats {
    pub capacity:    usize,
    pub used:        usize,
    pub hits:        u64,
    pub misses:      u64,
    pub evictions:   u64,
    pub total_bytes: u64,
    pub hit_rate_pct: u8,
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkCache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache de chunks thread-safe (UnsafeCell + spinlock AtomicU64).
pub struct ChunkCache {
    entries:    UnsafeCell<BTreeMap<[u8; 32], ChunkCacheEntry>>,
    lock:       AtomicU64,
    capacity:   usize,
    hits:       AtomicU64,
    misses:     AtomicU64,
    evictions:  AtomicU64,
    total_bytes: AtomicU64,
}

unsafe impl Sync for ChunkCache {}
unsafe impl Send for ChunkCache {}

impl ChunkCache {
    /// Constructeur const pour statique.
    pub const fn new_const(capacity: usize) -> Self {
        Self {
            entries:     UnsafeCell::new(BTreeMap::new()),
            lock:        AtomicU64::new(0),
            capacity,
            hits:        AtomicU64::new(0),
            misses:      AtomicU64::new(0),
            evictions:   AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
        }
    }

    pub fn new(capacity: usize) -> ExofsResult<Self> {
        if capacity == 0 || capacity > CHUNK_CACHE_MAX_CAPACITY {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self::new_const(capacity))
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn release(&self) { self.lock.store(0, Ordering::Release); }

    fn map(&self) -> &mut BTreeMap<[u8; 32], ChunkCacheEntry> {
        unsafe { &mut *self.entries.get() }
    }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Récupère les données d'un chunk par son blake3.
    pub fn get(&self, key: &[u8; 32]) -> Option<Vec<u8>> {
        self.acquire();
        let result = if let Some(entry) = self.map().get_mut(key) {
            entry.touch();
            let mut v: Vec<u8> = Vec::new();
            let _ = v.try_reserve(entry.data.len());
            v.extend_from_slice(&entry.data);
            Some(v)
        } else {
            None
        };
        self.release();
        if result.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Insère un chunk dans le cache.
    ///
    /// OOM-02 : try_reserve.
    /// ARITH-02 : saturating_add.
    pub fn insert(&self, fingerprint: ChunkFingerprint, data: &[u8]) -> ExofsResult<()> {
        let key       = fingerprint.blake3;
        let data_len  = data.len() as u64;
        let entry     = ChunkCacheEntry::new(fingerprint, data)?;
        self.acquire();
        let map  = self.map();
        // Si déjà présent — mise à jour simple.
        if map.contains_key(&key) {
            map.insert(key, entry);
            self.release();
            self.total_bytes.fetch_add(data_len, Ordering::Relaxed);
            return Ok(());
        }
        // Éviction si plein.
        if map.len() >= self.capacity {
            self.evict_one_locked(map);
        }
        map.insert(key, entry);
        self.release();
        self.total_bytes.fetch_add(data_len, Ordering::Relaxed);
        Ok(())
    }

    /// Retire une entrée du cache.
    pub fn remove(&self, key: &[u8; 32]) -> bool {
        self.acquire();
        let found = self.map().remove(key).is_some();
        self.release();
        found
    }

    /// Vide entièrement le cache.
    pub fn clear(&self) {
        self.acquire();
        self.map().clear();
        self.release();
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
    }

    /// Retourne le nombre d'entrées actuelles.
    pub fn len(&self) -> usize {
        self.acquire();
        let n = self.map().len();
        self.release();
        n
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Vérifie si une clé est dans le cache (sans accès).
    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.acquire();
        let found = self.map().contains_key(key);
        self.release();
        found
    }

    /// Statistiques du cache.
    pub fn stats(&self) -> ChunkCacheStats {
        let hits    = self.hits.load(Ordering::Relaxed);
        let misses  = self.misses.load(Ordering::Relaxed);
        let total   = hits.saturating_add(misses);
        let hit_pct = if total == 0 { 0 } else { (hits * 100 / total) as u8 };
        self.acquire();
        let used = self.map().len();
        self.release();
        ChunkCacheStats {
            capacity:    self.capacity,
            used,
            hits,
            misses,
            evictions:   self.evictions.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            hit_rate_pct: hit_pct,
        }
    }

    /// Éviction LRU approximatif : retire l'entrée avec le moins d'accès.
    ///
    /// RECUR-01 : boucle for — pas de récursion.
    fn evict_one_locked(&self, map: &mut BTreeMap<[u8; 32], ChunkCacheEntry>) {
        if map.is_empty() { return; }
        let mut min_key     = None::<[u8; 32]>;
        let mut min_access  = u64::MAX;
        for (k, v) in map.iter() {
            if v.accesses < min_access {
                min_access = v.accesses;
                min_key    = Some(*k);
            }
        }
        if let Some(k) = min_key {
            map.remove(&k);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Éviction forcée de plusieurs entrées LRU.
    pub fn evict_n(&self, n: usize) {
        if n == 0 { return; }
        self.acquire();
        for _ in 0..n {
            if self.map().is_empty() { break; }
            self.evict_one_locked(self.map());
        }
        self.release();
    }

    /// Vérifie l'intégrité des entrées : recalcule le blake3 des données.
    ///
    /// RECUR-01 : boucle for — pas de récursion.
    pub fn verify_integrity(&self) -> ExofsResult<()> {
        self.acquire();
        let mut bad = None::<[u8; 32]>;
        for (k, v) in self.map().iter() {
            let computed = blake3_hash(&v.data);
            if &computed != k {
                bad = Some(*k);
                break;
            }
        }
        self.release();
        if bad.is_some() {
            Err(ExofsError::CorruptedStructure)
        } else {
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statique global
// ─────────────────────────────────────────────────────────────────────────────

pub static CHUNK_CACHE: ChunkCache = ChunkCache::new_const(CHUNK_CACHE_DEFAULT_CAPACITY);

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};

    fn make_fp(data: &[u8]) -> ChunkFingerprint {
        ChunkFingerprint::compute(data, FingerprintAlgorithm::Double).unwrap()
    }

    #[test] fn test_cache_insert_get() {
        let c    = ChunkCache::new(64).unwrap();
        let data = b"hello world cache";
        let fp   = make_fp(data);
        c.insert(fp, data).unwrap();
        let got = c.get(&fp.blake3).unwrap();
        assert_eq!(got, data);
    }

    #[test] fn test_cache_miss() {
        let c   = ChunkCache::new(64).unwrap();
        let key = [0u8; 32];
        assert!(c.get(&key).is_none());
    }

    #[test] fn test_cache_stats_hits() {
        let c    = ChunkCache::new(64).unwrap();
        let data = b"stats test";
        let fp   = make_fp(data);
        c.insert(fp, data).unwrap();
        c.get(&fp.blake3);
        c.get(&fp.blake3);
        let s = c.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 0);
    }

    #[test] fn test_cache_eviction_on_full() {
        let cap  = 4usize;
        let c    = ChunkCache::new(cap).unwrap();
        for i in 0..cap + 1 {
            let data: Vec<u8> = (0..16).map(|b| b ^ i as u8).collect();
            let fp = make_fp(&data);
            c.insert(fp, &data).unwrap();
        }
        assert!(c.len() <= cap);
    }

    #[test] fn test_cache_clear() {
        let c    = ChunkCache::new(64).unwrap();
        let data = b"to be cleared";
        let fp   = make_fp(data);
        c.insert(fp, data).unwrap();
        c.clear();
        assert!(c.is_empty());
    }

    #[test] fn test_cache_remove() {
        let c    = ChunkCache::new(64).unwrap();
        let data = b"removable";
        let fp   = make_fp(data);
        c.insert(fp, data).unwrap();
        assert!(c.remove(&fp.blake3));
        assert!(c.get(&fp.blake3).is_none());
    }

    #[test] fn test_cache_contains() {
        let c    = ChunkCache::new(64).unwrap();
        let data = b"contains check";
        let fp   = make_fp(data);
        assert!(!c.contains(&fp.blake3));
        c.insert(fp, data).unwrap();
        assert!(c.contains(&fp.blake3));
    }

    #[test] fn test_invalid_capacity() {
        assert!(ChunkCache::new(0).is_err());
        assert!(ChunkCache::new(CHUNK_CACHE_MAX_CAPACITY + 1).is_err());
    }

    #[test] fn test_evict_n() {
        let c = ChunkCache::new(64).unwrap();
        for i in 0u8..10 {
            let data = [i; 32];
            let fp = make_fp(&data);
            c.insert(fp, &data).unwrap();
        }
        let before = c.len();
        c.evict_n(3);
        assert!(c.len() <= before - 3);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkCacheConfig — configuration avancée du cache
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration complète du cache de chunks.
#[derive(Debug, Clone, Copy)]
pub struct ChunkCacheConfig {
    pub capacity:   usize,
    pub max_bytes:  u64,
    pub verify_on_get: bool,
}

impl ChunkCacheConfig {
    pub fn default() -> Self {
        Self {
            capacity:      CHUNK_CACHE_DEFAULT_CAPACITY,
            max_bytes:     CHUNK_CACHE_MAX_BYTES,
            verify_on_get: false,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.capacity == 0 || self.capacity > CHUNK_CACHE_MAX_CAPACITY {
            return Err(ExofsError::InvalidArgument);
        }
        if self.max_bytes == 0 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

#[cfg(test)]
mod tests_config {
    use super::*;

    #[test] fn test_config_default_valid() {
        assert!(ChunkCacheConfig::default().validate().is_ok());
    }

    #[test] fn test_config_zero_capacity_invalid() {
        let c = ChunkCacheConfig { capacity: 0, ..ChunkCacheConfig::default() };
        assert!(c.validate().is_err());
    }

    #[test] fn test_global_cache_accessible() {
        let s = CHUNK_CACHE.stats();
        assert_eq!(s.capacity, CHUNK_CACHE_DEFAULT_CAPACITY);
    }
}
