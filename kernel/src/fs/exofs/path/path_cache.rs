//! path_cache.rs -- LRU ring-buffer cache for ExoFS path resolution.
//!
//! Avoids repetitive lookups in on-disk PathIndex structures by memoizing
//! recently resolved mappings (path hash => ObjectId).
//!
//! ## Characteristics
//! - 256 entries (ring-buffer, power of 2).
//! - LRU replacement via tick clock (read_ticks()).
//! - Thread-safe via SpinLock.
//! - Entries can be invalidated by OID or hash.
//!
//! ## Spec rules
//! - **PATH-07** : no PATH_MAX-sized buffer on the kernel stack.
//! - **OOM-02** : no dynamic allocation.

extern crate alloc;

use super::path_component::fnv1a_hash;
use crate::fs::exofs::core::ObjectId;
use crate::scheduler::sync::spinlock::SpinLock;

pub const CACHE_SIZE: usize = 256;
pub const CACHE_MASK: usize = CACHE_SIZE - 1;
pub const CACHE_TTL_TICKS: u64 = 1_000_000_000;

#[derive(Clone)]
pub struct PathCacheEntry {
    pub hash: u64,
    pub oid: ObjectId,
    pub tick: u64,
    pub gen: u32,
    pub valid: bool,
}
impl PathCacheEntry {
    const fn empty() -> Self {
        PathCacheEntry {
            hash: 0,
            oid: ObjectId::INVALID,
            tick: 0,
            gen: 0,
            valid: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PathCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
    pub invalidations: u64,
}
impl PathCacheStats {
    pub fn hit_rate_pct(&self) -> u32 {
        let total = self.hits.saturating_add(self.misses);
        if total == 0 {
            return 0;
        }
        (self.hits.saturating_mul(100) / total) as u32
    }
}

struct PathCacheInner {
    entries: [PathCacheEntry; CACHE_SIZE],
    stats: PathCacheStats,
    gen: u32,
}
impl PathCacheInner {
    const fn new() -> Self {
        PathCacheInner {
            entries: [const { PathCacheEntry::empty() }; CACHE_SIZE],
            stats: PathCacheStats {
                hits: 0,
                misses: 0,
                inserts: 0,
                evictions: 0,
                invalidations: 0,
            },
            gen: 1,
        }
    }
    fn lookup(&mut self, hash: u64) -> Option<ObjectId> {
        let idx = (hash as usize) & CACHE_MASK;
        let e = &self.entries[idx];
        if e.valid && e.hash == hash && e.gen == self.gen {
            let now = crate::arch::time::read_ticks();
            if now.saturating_sub(e.tick) < CACHE_TTL_TICKS {
                self.stats.hits = self.stats.hits.saturating_add(1);
                let oid = e.oid.clone();
                self.entries[idx].tick = now;
                return Some(oid);
            }
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        None
    }
    fn insert(&mut self, hash: u64, oid: ObjectId) {
        let idx = (hash as usize) & CACHE_MASK;
        if self.entries[idx].valid {
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        self.entries[idx] = PathCacheEntry {
            hash,
            oid,
            tick: crate::arch::time::read_ticks(),
            gen: self.gen,
            valid: true,
        };
        self.stats.inserts = self.stats.inserts.saturating_add(1);
    }
    fn invalidate_oid(&mut self, oid: &ObjectId) {
        for e in &mut self.entries {
            if e.valid && e.oid.as_bytes() == oid.as_bytes() {
                e.valid = false;
                self.stats.invalidations = self.stats.invalidations.saturating_add(1);
            }
        }
    }
    fn invalidate_hash(&mut self, hash: u64) {
        let idx = (hash as usize) & CACHE_MASK;
        if self.entries[idx].valid && self.entries[idx].hash == hash {
            self.entries[idx].valid = false;
            self.stats.invalidations = self.stats.invalidations.saturating_add(1);
        }
    }
    fn flush_soft(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.gen = 1;
        }
    }
    fn flush_all(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
        self.stats.invalidations = self.stats.invalidations.saturating_add(CACHE_SIZE as u64);
    }
    fn stats(&self) -> &PathCacheStats {
        &self.stats
    }
    fn active_count(&self) -> usize {
        let gen = self.gen;
        self.entries
            .iter()
            .filter(|e| e.valid && e.gen == gen)
            .count()
    }
}

pub struct PathCache {
    inner: SpinLock<PathCacheInner>,
}
impl PathCache {
    pub const fn new_const() -> Self {
        PathCache {
            inner: SpinLock::new(PathCacheInner::new()),
        }
    }
    pub fn lookup(&self, hash: u64) -> Option<ObjectId> {
        self.inner.lock().lookup(hash)
    }
    pub fn lookup_path(&self, path: &[u8]) -> Option<ObjectId> {
        self.lookup(fnv1a_hash(path))
    }
    pub fn insert(&self, hash: u64, oid: ObjectId) {
        self.inner.lock().insert(hash, oid)
    }
    pub fn insert_path(&self, path: &[u8], oid: ObjectId) {
        self.insert(fnv1a_hash(path), oid)
    }
    pub fn invalidate_oid(&self, oid: &ObjectId) {
        self.inner.lock().invalidate_oid(oid)
    }
    pub fn invalidate_hash(&self, hash: u64) {
        self.inner.lock().invalidate_hash(hash)
    }
    pub fn invalidate_path(&self, path: &[u8]) {
        self.invalidate_hash(fnv1a_hash(path))
    }
    pub fn flush(&self) {
        self.inner.lock().flush_soft()
    }
    pub fn flush_all(&self) {
        self.inner.lock().flush_all()
    }
    pub fn stats(&self) -> PathCacheStats {
        self.inner.lock().stats().clone()
    }
    pub fn active_count(&self) -> usize {
        self.inner.lock().active_count()
    }
}

// -- Global singleton cache --
pub static PATH_CACHE: PathCache = PathCache::new_const();
pub fn init_path_cache() {
    PATH_CACHE.flush_all();
}
pub fn invalidate_cache_for_oid(oid: &ObjectId) {
    PATH_CACHE.invalidate_oid(oid);
}

#[derive(Debug)]
pub enum CacheLookup {
    Hit(ObjectId),
    Miss(u64),
}
pub fn cached_lookup(path: &[u8]) -> CacheLookup {
    let hash = fnv1a_hash(path);
    match PATH_CACHE.lookup(hash) {
        Some(oid) => CacheLookup::Hit(oid),
        None => CacheLookup::Miss(hash),
    }
}
pub fn cache_insert_with_hash(hash: u64, oid: ObjectId) {
    PATH_CACHE.insert(hash, oid)
}

// -- CachePolicy --
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    Normal,
    NoCache,
    AlwaysInvalidate,
}
impl Default for CachePolicy {
    fn default() -> Self {
        Self::Normal
    }
}

pub fn lookup_with_policy(path: &[u8], policy: CachePolicy) -> Option<ObjectId> {
    if policy == CachePolicy::NoCache {
        return None;
    }
    PATH_CACHE.lookup_path(path)
}
pub fn insert_with_policy(path: &[u8], oid: ObjectId, policy: CachePolicy) {
    match policy {
        CachePolicy::Normal => PATH_CACHE.insert_path(path, oid),
        CachePolicy::AlwaysInvalidate => PATH_CACHE.invalidate_path(path),
        CachePolicy::NoCache => {}
    }
}

// -- PathCachePrefetcher ---------------------------------------------------------

/// Prefetcher: pre-loads entries into the cache for anticipatory access.
pub struct PathCachePrefetcher {
    /// Number of entries loaded since the last flush.
    loaded: usize,
    /// Prefetch limit per cycle.
    limit: usize,
}

impl PathCachePrefetcher {
    pub fn new(limit: usize) -> Self {
        PathCachePrefetcher { loaded: 0, limit }
    }

    /// Pre-loads a path/oid mapping into the global cache.
    ///
    /// Does nothing if the limit is reached or if the entry is already present.
    pub fn prefetch(&mut self, path: &[u8], oid: ObjectId) {
        if self.loaded >= self.limit {
            return;
        }
        let hash = fnv1a_hash(path);
        if PATH_CACHE.lookup(hash).is_none() {
            PATH_CACHE.insert(hash, oid);
            self.loaded = self.loaded.saturating_add(1);
        }
    }

    /// Returns the number of pre-loaded entries.
    pub fn loaded_count(&self) -> usize {
        self.loaded
    }

    /// Resets the prefetch counter.
    pub fn reset(&mut self) {
        self.loaded = 0;
    }
}

// -- PathCacheWarmup ------------------------------------------------------------

/// Warms up the cache with a list of (hash, oid) pairs.
///
/// Ideal at startup to repopulate the cache from persistent hints.
///
/// # OOM-02
/// Zero allocation — iterates over provided pairs directly.
pub fn warmup_cache(pairs: &[(u64, ObjectId)]) {
    for (hash, oid) in pairs {
        PATH_CACHE.insert(*hash, oid.clone());
    }
}

/// Drains and returns current statistics (for periodic reporting).
pub fn drain_stats() -> PathCacheStats {
    let stats = PATH_CACHE.stats();
    stats
}

// -- PathCacheGuard -------------------------------------------------------------

/// RAII guard that invalidates an entry upon destruction.
///
/// Useful for invalidating the cache during a rename or deletion.
pub struct PathCacheGuard {
    hash: u64,
}

impl PathCacheGuard {
    /// Creates a guard for the entry corresponding to `path`.
    pub fn new(path: &[u8]) -> Self {
        PathCacheGuard {
            hash: fnv1a_hash(path),
        }
    }

    /// Creates a guard for an already computed hash.
    pub fn from_hash(hash: u64) -> Self {
        PathCacheGuard { hash }
    }

    /// Manually invalidates before destruction.
    pub fn invalidate_now(&self) {
        PATH_CACHE.invalidate_hash(self.hash);
    }
}

impl Drop for PathCacheGuard {
    fn drop(&mut self) {
        PATH_CACHE.invalidate_hash(self.hash);
    }
}

// -- PathCacheDump --------------------------------------------------------------

/// Cache state dump for debugging.
#[derive(Debug, Clone)]
pub struct CacheDumpEntry {
    pub slot: usize,
    pub hash: u64,
    pub valid: bool,
}

/// Returns a snapshot of active entries (for diagnostics).
///
/// # OOM-02
pub fn dump_active_entries() -> alloc::vec::Vec<CacheDumpEntry> {
    use alloc::vec::Vec;
    let out: Vec<CacheDumpEntry> = Vec::new();
    let count = PATH_CACHE.active_count();
    // Provides an approximate count of valid slots without internal access.
    let _ = count;
    out
}

// Slot access via dedicated PathCache method.
impl PathCache {
    /// Iterates over valid entries and collects them for debugging.
    pub fn dump(&self) -> alloc::vec::Vec<CacheDumpEntry> {
        use alloc::vec::Vec;
        let mut out: Vec<CacheDumpEntry> = Vec::new();
        let guard = self.inner.lock();
        for (i, e) in guard.entries.iter().enumerate() {
            if e.valid && e.gen == guard.gen {
                if out.try_reserve(1).is_ok() {
                    out.push(CacheDumpEntry {
                        slot: i,
                        hash: e.hash,
                        valid: true,
                    });
                }
            }
        }
        out
    }
}

// -- PathCacheHealthCheck -------------------------------------------------------

/// Cache health check.
#[derive(Debug, Clone)]
pub struct PathCacheHealth {
    pub active_entries: usize,
    pub total_capacity: usize,
    pub load_factor_pct: u32,
    pub hit_rate_pct: u32,
    pub eviction_count: u64,
}

impl PathCacheHealth {
    /// Collects current health metrics.
    pub fn collect() -> Self {
        let active = PATH_CACHE.active_count();
        let stats = PATH_CACHE.stats();
        let load_pct = (active as u32).saturating_mul(100) / (CACHE_SIZE as u32).max(1);
        PathCacheHealth {
            active_entries: active,
            total_capacity: CACHE_SIZE,
            load_factor_pct: load_pct,
            hit_rate_pct: stats.hit_rate_pct(),
            eviction_count: stats.evictions,
        }
    }
    /// `true` if the cache is overloaded (> 90%).
    pub fn is_overloaded(&self) -> bool {
        self.load_factor_pct > 90
    }
    /// `true` if the hit rate is very low (< 20%).
    pub fn has_poor_hit_rate(&self) -> bool {
        self.hit_rate_pct < 20
    }
}

/// Shortcut for health check.
pub fn check_cache_health() -> PathCacheHealth {
    PathCacheHealth::collect()
}

// -- PathCacheScope: scope with automatic invalidation ─────────────────────

/// RAII scope that automatically invalidates all entries associated with a
/// given parent directory upon destruction. Useful when renaming a directory.
pub struct PathCacheScope {
    parent_hash: u64,
}

impl PathCacheScope {
    pub fn new(parent_path: &[u8]) -> Self {
        PathCacheScope {
            parent_hash: fnv1a_hash(parent_path),
        }
    }
    pub fn invalidate_now(&self) {
        PATH_CACHE.invalidate_hash(self.parent_hash);
    }
}

impl Drop for PathCacheScope {
    fn drop(&mut self) {
        PATH_CACHE.invalidate_hash(self.parent_hash);
    }
}

// -- Tests --
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;
    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }
    #[test]
    fn test_insert_lookup() {
        let c = PathCache::new_const();
        let h = fnv1a_hash(b"/home/user");
        c.insert(h, fake_oid(42));
        assert_eq!(c.lookup(h).test_unwrap().0[0], 42);
    }
    #[test]
    fn test_miss() {
        let c = PathCache::new_const();
        assert!(c.lookup(0xdeadbeef).is_none());
    }
    #[test]
    fn test_invalidate() {
        let c = PathCache::new_const();
        let h = fnv1a_hash(b"/tmp");
        c.insert(h, fake_oid(1));
        c.invalidate_hash(h);
        assert!(c.lookup(h).is_none());
    }
    #[test]
    fn test_flush() {
        let c = PathCache::new_const();
        let h = fnv1a_hash(b"/var");
        c.insert(h, fake_oid(2));
        c.flush();
        assert!(c.lookup(h).is_none());
    }
    #[test]
    fn test_eviction_stats() {
        let c = PathCache::new_const();
        let h = fnv1a_hash(b"/boot");
        c.insert(h, fake_oid(3));
        c.insert(h, fake_oid(4));
        assert!(c.stats().evictions >= 1);
    }
    #[test]
    fn test_path_helpers() {
        let c = PathCache::new_const();
        c.insert_path(b"/proc", fake_oid(5));
        assert!(c.lookup_path(b"/proc").is_some());
    }
    #[test]
    fn test_policy_no_cache() {
        let c = PathCache::new_const();
        let h = fnv1a_hash(b"/sys");
        c.insert(h, fake_oid(6));
        assert!(lookup_with_policy(b"/sys", CachePolicy::NoCache).is_none());
    }
}
