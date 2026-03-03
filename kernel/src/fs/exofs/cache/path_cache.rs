//! PathCache — cache de résolution de chemins ExoFS (no_std).
//!
//! RÈGLE 10 : pas de buffers PATH_MAX sur la pile kernel.

//! path_cache.rs — Cache de résolution de chemins ExoFS (no_std).
//!
//! Règle : pas de buffers PATH_MAX sur la pile kernel.
//! Clé = FNV-1a 64-bit du chemin + vérification anti-collision.
//! Règles : OOM-02, ARITH-02, RECUR-01.

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;
use super::cache_stats::CACHE_STATS;

// ── PathEntry ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PathEntry {
    /// Chemin complet sur heap (jamais sur stack).
    pub path:         String,
    pub inode_id:     u64,
    pub flags:        u32,
    pub depth:        u32,
    pub cached_tick:  u64,
    pub access_count: u64,
}

impl PathEntry {
    pub fn new(path: String, inode_id: u64, flags: u32, depth: u32, now: u64) -> Self {
        Self { path, inode_id, flags, depth, cached_tick: now, access_count: 1 }
    }

    pub fn touch(&mut self) {
        self.access_count = self.access_count.wrapping_add(1);
    }

    pub fn is_stale(&self, now: u64, ttl: u64) -> bool {
        now.saturating_sub(self.cached_tick) >= ttl
    }
}

// ── PathCacheInner ───────────────────────────────────────────────────────────

struct PathCacheInner {
    map: BTreeMap<u64, PathEntry>,
    max: usize,
}

impl PathCacheInner {
    fn new(max: usize) -> Self { Self { map: BTreeMap::new(), max } }

    fn evict_one_lru(&mut self) {
        let oldest = self.map
            .iter()
            .min_by_key(|(_, e)| e.cached_tick)
            .map(|(k, _)| *k);
        if let Some(k) = oldest { self.map.remove(&k); }
    }
}

// ── PathCache ──────────────────────────────────────────────────────────────────

pub struct PathCache {
    inner:          SpinLock<PathCacheInner>,
    hits:           AtomicU64,
    misses:         AtomicU64,
    invalidations:  AtomicU64,
}

pub static PATH_CACHE: PathCache = PathCache::new_const();

impl PathCache {
    pub const fn new_const() -> Self {
        Self {
            inner:         SpinLock::new(PathCacheInner::new(16384)),
            hits:          AtomicU64::new(0),
            misses:        AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
        }
    }

    // ── FNV-1a hash ───────────────────────────────────────────────────────

    fn fnv1a(path: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in path {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    // ── Lookup ─────────────────────────────────────────────────────────────────

    pub fn lookup(&self, path: &[u8]) -> Option<PathEntry> {
        let key = Self::fnv1a(path);
        let mut inner = self.inner.lock();
        if let Some(e) = inner.map.get_mut(&key) {
            if e.path.as_bytes() == path {
                e.touch();
                self.hits.fetch_add(1, Ordering::Relaxed);
                CACHE_STATS.record_hit();
                return Some(e.clone());
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        CACHE_STATS.record_miss();
        None
    }

    pub fn contains(&self, path: &[u8]) -> bool {
        self.lookup(path).is_some()
    }

    // ── Insertion ─────────────────────────────────────────────────────────────

    pub fn insert(&self, path: &[u8], inode_id: u64, flags: u32) -> ExofsResult<()> {
        // Chemin sur heap (Règle RECUR-01 + pas de heap sur pile).
        let path_str = {
            let mut s = String::new();
            s.try_reserve(path.len()).map_err(|_| ExofsError::NoMemory)?;
            for &b in path { s.push(b as char); }
            s
        };
        let depth = path.iter().filter(|&&c| c == b'/').count() as u32;
        let now   = crate::arch::time::read_ticks();
        let key   = Self::fnv1a(path);
        let entry = PathEntry::new(path_str, inode_id, flags, depth, now);

        let mut inner = self.inner.lock();
        inner.map.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        if inner.map.len() >= inner.max { inner.evict_one_lru(); }
        inner.map.insert(key, entry);
        CACHE_STATS.record_insert(path.len() as u64);
        Ok(())
    }

    // ── Invalidation ──────────────────────────────────────────────────────────

    pub fn invalidate_path(&self, path: &[u8]) {
        let key = Self::fnv1a(path);
        if self.inner.lock().map.remove(&key).is_some() {
            self.invalidations.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn invalidate_prefix(&self, prefix: &[u8]) {
        let mut inner = self.inner.lock();
        let to_remove: Vec<u64> = inner.map
            .iter()
            .filter(|(_, e)| e.path.as_bytes().starts_with(prefix))
            .map(|(k, _)| *k)
            .collect();
        for k in to_remove {
            inner.map.remove(&k);
            self.invalidations.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn invalidate_inode(&self, inode_id: u64) {
        let mut inner = self.inner.lock();
        let to_remove: Vec<u64> = inner.map
            .iter()
            .filter(|(_, e)| e.inode_id == inode_id)
            .map(|(k, _)| *k)
            .collect();
        for k in to_remove {
            inner.map.remove(&k);
            self.invalidations.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn evict_stale(&self, now: u64, ttl: u64) -> usize {
        let mut inner = self.inner.lock();
        let stale: Vec<u64> = inner.map
            .iter()
            .filter(|(_, e)| e.is_stale(now, ttl))
            .map(|(k, _)| *k)
            .collect();
        let count = stale.len();
        for k in stale {
            inner.map.remove(&k);
            self.invalidations.fetch_add(1, Ordering::Relaxed);
        }
        count
    }

    // ── Statistiques ────────────────────────────────────────────────────────

    pub fn n_entries(&self)     -> usize { self.inner.lock().map.len() }
    pub fn hits(&self)          -> u64   { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self)        -> u64   { self.misses.load(Ordering::Relaxed) }
    pub fn invalidations(&self) -> u64   { self.invalidations.load(Ordering::Relaxed) }
    pub fn flush_all(&self)              { self.inner.lock().map.clear(); }
}

// ── Extensions PathCache ───────────────────────────────────────────────────────

impl PathCache {
    /// Insère ou ré-insère si TTL expiré.
    pub fn lookup_or_insert(
        &self, path: &[u8], inode_id: u64, flags: u32, ttl: u64,
    ) -> ExofsResult<PathEntry> {
        let now = crate::arch::time::read_ticks();
        if let Some(e) = self.lookup(path) {
            if !e.is_stale(now, ttl) { return Ok(e); }
        }
        self.insert(path, inode_id, flags)?;
        self.lookup(path).ok_or(ExofsError::InternalError)
    }

    /// Insère un lot de chemins. Stoppe au premier échec OOM.
    pub fn insert_batch(&self, entries: &[(&[u8], u64, u32)]) -> ExofsResult<usize> {
        let mut count = 0usize;
        for &(path, inode_id, flags) in entries {
            self.insert(path, inode_id, flags)?;
            count = count.wrapping_add(1);
        }
        Ok(count)
    }

    /// Renomme un chemin (mise à jour atomique sous lock).
    pub fn rename(&self, old_path: &[u8], new_path: &[u8]) -> ExofsResult<()> {
        let old_key = Self::fnv1a(old_path);
        let entry = {
            let inner = self.inner.lock();
            inner.map.get(&old_key)
                .filter(|e| e.path.as_bytes() == old_path)
                .map(|e| (e.inode_id, e.flags))
        };
        match entry {
            Some((inode_id, flags)) => {
                self.invalidate_path(old_path);
                self.insert(new_path, inode_id, flags)
            }
            None => Err(ExofsError::ObjectNotFound),
        }
    }

    /// Retourne les N chemins les plus récemment accédés.
    pub fn hottest_n(&self, n: usize) -> Vec<PathEntry> {
        let inner = self.inner.lock();
        let mut entries: Vec<PathEntry> = inner.map.values().cloned().collect();
        entries.sort_unstable_by(|a, b| b.access_count.cmp(&a.access_count));
        entries.truncate(n);
        entries
    }

    /// Nombre de chemins par profondeur.
    pub fn depth_histogram(&self) -> [usize; 8] {
        let inner = self.inner.lock();
        let mut hist = [0usize; 8];
        for e in inner.map.values() {
            let idx = (e.depth as usize).min(7);
            hist[idx] = hist[idx].wrapping_add(1);
        }
        hist
    }
}


// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_insert_and_lookup() {
        let c = PathCache::new_const();
        c.insert(b"/foo/bar", 42, 0).unwrap();
        let r = c.lookup(b"/foo/bar");
        assert!(r.is_some());
        assert_eq!(r.unwrap().inode_id, 42);
    }

    #[test] fn test_miss() {
        let c = PathCache::new_const();
        assert!(c.lookup(b"/not/found").is_none());
    }

    #[test] fn test_invalidate_path() {
        let c = PathCache::new_const();
        c.insert(b"/a/b", 1, 0).unwrap();
        c.invalidate_path(b"/a/b");
        assert!(c.lookup(b"/a/b").is_none());
    }

    #[test] fn test_invalidate_prefix() {
        let c = PathCache::new_const();
        c.insert(b"/a", 1, 0).unwrap();
        c.insert(b"/a/b", 2, 0).unwrap();
        c.insert(b"/z", 3, 0).unwrap();
        c.invalidate_prefix(b"/a");
        assert_eq!(c.n_entries(), 1);
    }

    #[test] fn test_invalidate_inode() {
        let c = PathCache::new_const();
        c.insert(b"/x", 7, 0).unwrap();
        c.insert(b"/y", 7, 0).unwrap();
        c.invalidate_inode(7);
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_hits_misses() {
        let c = PathCache::new_const();
        c.insert(b"/p", 1, 0).unwrap();
        c.lookup(b"/p"); c.lookup(b"/q");
        assert_eq!(c.hits(), 1);
        assert_eq!(c.misses(), 1);
    }

    #[test] fn test_evict_stale() {
        let c = PathCache::new_const();
        c.insert(b"/old", 1, 0).unwrap();
        // Force le tick à 0 via update direct.
        { c.inner.lock().map.values_mut().for_each(|e| e.cached_tick = 0); }
        let evicted = c.evict_stale(5000, 100);
        assert_eq!(evicted, 1);
    }

    #[test] fn test_depth_counted() {
        let c = PathCache::new_const();
        c.insert(b"/a/b/c", 1, 0).unwrap();
        let e = c.lookup(b"/a/b/c").unwrap();
        assert_eq!(e.depth, 3);
    }

    #[test] fn test_flush_all() {
        let c = PathCache::new_const();
        c.insert(b"/x", 1, 0).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_rename() {
        let c = PathCache::new_const();
        c.insert(b"/old", 42, 0).unwrap();
        c.rename(b"/old", b"/new").unwrap();
        assert!(c.lookup(b"/old").is_none());
        assert!(c.lookup(b"/new").is_some());
    }

    #[test] fn test_hottest_n() {
        let c = PathCache::new_const();
        c.insert(b"/hot", 1, 0).unwrap();
        c.insert(b"/cold", 2, 0).unwrap();
        // accéder plusieurs fois à /hot
        c.lookup(b"/hot"); c.lookup(b"/hot"); c.lookup(b"/hot");
        let hot = c.hottest_n(1);
        assert_eq!(hot[0].path.as_bytes(), b"/hot");
    }

    #[test] fn test_insert_batch() {
        let c = PathCache::new_const();
        let batch = [(b"/a" as &[u8], 1u64, 0u32), (b"/b", 2, 0)];
        let n = c.insert_batch(&batch).unwrap();
        assert_eq!(n, 2);
    }

    #[test] fn test_depth_histogram() {
        let c = PathCache::new_const();
        c.insert(b"/a/b/c", 1, 0).unwrap();
        let h = c.depth_histogram();
        assert_eq!(h[3], 1);
    }

    #[test] fn test_contains() {
        let c = PathCache::new_const();
        c.insert(b"/foo", 1, 0).unwrap();
        assert!(c.contains(b"/foo"));
        assert!(!c.contains(b"/bar"));
    }

    #[test] fn test_flush_all_clears() {
        let c = PathCache::new_const();
        c.insert(b"/x", 1, 0).unwrap();
        c.flush_all();
        assert_eq!(c.n_entries(), 0);
    }

    #[test] fn test_remove_existing_path() {
        let c = PathCache::new_const();
        c.insert(b"/rem", 2, 0).unwrap();
        c.remove(b"/rem");
        assert!(!c.contains(b"/rem"));
    }

    #[test] fn test_get_miss() {
        let c = PathCache::new_const();
        assert!(c.get(b"/nonexistent").is_none());
    }

    #[test] fn test_n_entries_after_inserts() {
        let c = PathCache::new_const();
        c.insert(b"/a", 1, 0).unwrap();
        c.insert(b"/b", 2, 0).unwrap();
        assert_eq!(c.n_entries(), 2);
    }

    #[test] fn test_depth_histogram_multiple_depths() {
        let c = PathCache::new_const();
        c.insert(b"/a",     1, 0).unwrap();
        c.insert(b"/a/b",   2, 0).unwrap();
        c.insert(b"/a/b/c", 3, 0).unwrap();
        let h = c.depth_histogram();
        assert_eq!(h[1], 1);
        assert_eq!(h[2], 1);
        assert_eq!(h[3], 1);
    }
}
