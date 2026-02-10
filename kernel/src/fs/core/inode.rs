//! Inode Management
//!
//! Provides inode cache and management functions for the VFS layer.

use alloc::sync::Arc;
use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::RwLock;

use crate::fs::{FsError, FsResult};
use super::types::{Inode, InodeStat, InodeType};

// ═══════════════════════════════════════════════════════════════════════════
// INODE CACHE
// ═══════════════════════════════════════════════════════════════════════════

/// Inode Cache - Maps inode numbers to cached inode objects
///
/// ## Performance Targets
/// - Lookup: O(1) average via HashMap
/// - Insert: O(1) average
/// - Eviction: LRU when cache is full
pub struct InodeCache {
    /// Inode number → Inode trait object
    cache: RwLock<HashMap<u64, Arc<dyn Inode>>>,
    /// Maximum entries
    max_entries: usize,
}

impl InodeCache {
    /// Create new inode cache
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(HashMap::with_capacity(max_entries)),
            max_entries,
        }
    }

    /// Lookup inode by number
    ///
    /// # Performance
    /// - Target: < 50ns for cache hit
    #[inline]
    pub fn get(&self, ino: u64) -> Option<Arc<dyn Inode>> {
        let cache = self.cache.read();
        cache.get(&ino).cloned()
    }

    /// Insert inode into cache
    ///
    /// # Performance
    /// - Target: < 150ns
    pub fn insert(&self, ino: u64, inode: Arc<dyn Inode>) {
        let mut cache = self.cache.write();

        // Evict if cache is full
        if cache.len() >= self.max_entries {
            self.evict_one_locked(&mut cache);
        }

        cache.insert(ino, inode);
    }

    /// Remove inode from cache
    pub fn remove(&self, ino: u64) {
        let mut cache = self.cache.write();
        cache.remove(&ino);
    }

    /// Clear entire cache
    pub fn clear(&self) {
        let mut cache = self.cache.write();
        cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> InodeCacheStats {
        let cache = self.cache.read();
        InodeCacheStats {
            entries: cache.len(),
            max_entries: self.max_entries,
            hit_rate: 0.0, // TODO: Track hits/misses
        }
    }

    /// Evict one entry (simple: remove first)
    ///
    /// TODO: Implement proper LRU eviction
    fn evict_one_locked(&self, cache: &mut HashMap<u64, Arc<dyn Inode>>) {
        if let Some(&ino) = cache.keys().next() {
            cache.remove(&ino);
        }
    }

    /// Get number of cached inodes
    pub fn len(&self) -> usize {
        let cache = self.cache.read();
        cache.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InodeCache {
    fn default() -> Self {
        Self::new(10_000) // 10k inodes by default
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// STATISTICS
// ═══════════════════════════════════════════════════════════════════════════

/// Inode cache statistics
#[derive(Debug, Clone, Copy)]
pub struct InodeCacheStats {
    /// Current number of cached inodes
    pub entries: usize,
    /// Maximum entries allowed
    pub max_entries: usize,
    /// Cache hit rate (0.0 - 1.0)
    pub hit_rate: f32,
}

// ═══════════════════════════════════════════════════════════════════════════
// GLOBAL INSTANCE
// ═══════════════════════════════════════════════════════════════════════════

use spin::Once;

/// Global inode cache instance
pub static INODE_CACHE: Once<InodeCache> = Once::new();

/// Initialize global inode cache
pub fn init(max_entries: usize) {
    INODE_CACHE.call_once(|| InodeCache::new(max_entries));
}

/// Get global inode cache
#[inline]
pub fn get() -> &'static InodeCache {
    INODE_CACHE.get().expect("Inode cache not initialized")
}

// ═══════════════════════════════════════════════════════════════════════════
// HELPER FUNCTIONS
// ═══════════════════════════════════════════════════════════════════════════

/// Get inode from cache or error
pub fn get_inode(ino: u64) -> FsResult<Arc<dyn Inode>> {
    get().get(ino).ok_or(FsError::NoSuchFileOrDirectory)
}

/// Cache an inode
pub fn cache_inode(ino: u64, inode: Arc<dyn Inode>) {
    get().insert(ino, inode);
}

/// Remove inode from cache
pub fn uncache_inode(ino: u64) {
    get().remove(ino);
}
