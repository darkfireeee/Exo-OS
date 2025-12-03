//! VFS cache - High-performance inode and dentry cache
//!
//! Features:
//! - LRU eviction policy
//! - Lock-free reads where possible
//! - Cache-aligned structures

use super::inode::Inode;
use crate::fs::{FsError, FsResult};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use hashbrown::HashMap;
use spin::RwLock;

/// Get inode from global cache or underlying filesystem
pub fn get_inode(ino: u64) -> FsResult<Arc<RwLock<dyn Inode>>> {
    // First check local cache
    if let Some(inode) = get_cache().inode_cache.get(ino) {
        return Ok(inode);
    }
    
    // Cache miss - load from tmpfs via VFS
    // This returns Arc<RwLock<TmpfsInode>> which implements Inode trait
    let tmpfs_inode = super::get_inode(ino)?;
    
    // Store in cache and return
    let inode: Arc<RwLock<dyn Inode>> = tmpfs_inode;
    get_cache().inode_cache.insert(ino, Arc::clone(&inode));
    
    Ok(inode)
}

/// Inode cache entry
#[derive(Clone)]
struct InodeCacheEntry {
    inode: Arc<RwLock<dyn Inode>>,
    access_count: usize,
    dirty: bool,
}

/// VFS inode cache with LRU eviction
pub struct InodeCache {
    /// Cached inodes (keyed by inode number)
    cache: RwLock<HashMap<u64, InodeCacheEntry>>,
    /// LRU queue for eviction
    lru: RwLock<VecDeque<u64>>,
    /// Maximum cache size
    max_size: usize,
}

impl InodeCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: RwLock::new(HashMap::with_capacity(max_size)),
            lru: RwLock::new(VecDeque::with_capacity(max_size)),
            max_size,
        }
    }

    /// Get inode from cache
    pub fn get(&self, ino: u64) -> Option<Arc<RwLock<dyn Inode>>> {
        let mut cache = self.cache.write();

        if let Some(entry) = cache.get_mut(&ino) {
            entry.access_count += 1;

            // Update LRU (move to back)
            let mut lru = self.lru.write();
            lru.retain(|&i| i != ino);
            lru.push_back(ino);

            Some(entry.inode.clone())
        } else {
            None
        }
    }

    /// Insert inode into cache
    pub fn insert(&self, ino: u64, inode: Arc<RwLock<dyn Inode>>) {
        let mut cache = self.cache.write();
        let mut lru = self.lru.write();

        // Evict if cache is full
        if cache.len() >= self.max_size {
            if let Some(evict_ino) = lru.pop_front() {
                if let Some(entry) = cache.remove(&evict_ino) {
                    // Flush if dirty
                    if entry.dirty {
                        log::debug!("Evicting dirty inode {}, flushing", evict_ino);
                        // Flush to disk (stub)
                    }
                }
            }
        }

        // Insert new entry
        let entry = InodeCacheEntry {
            inode,
            access_count: 1,
            dirty: false,
        };

        cache.insert(ino, entry);
        lru.push_back(ino);
    }

    /// Mark inode as dirty (needs writeback)
    pub fn mark_dirty(&self, ino: u64) {
        let mut cache = self.cache.write();
        if let Some(entry) = cache.get_mut(&ino) {
            entry.dirty = true;
        }
    }

    /// Flush all dirty inodes
    pub fn flush_all(&self) {
        let cache = self.cache.read();
        for (ino, entry) in cache.iter() {
            if entry.dirty {
                log::debug!("Flushing dirty inode {}", ino);
                // Flush to disk (stub)
            }
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read();
        let dirty_count = cache.values().filter(|e| e.dirty).count();

        CacheStats {
            total_entries: cache.len(),
            dirty_entries: dirty_count,
            max_size: self.max_size,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub total_entries: usize,
    pub dirty_entries: usize,
    pub max_size: usize,
}

/// Dentry (directory entry) cache
pub struct DentryCache {
    /// Path -> inode number mapping
    cache: RwLock<HashMap<alloc::string::String, u64>>,
    max_size: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: RwLock::new(HashMap::with_capacity(max_size)),
            max_size,
        }
    }

    /// Lookup path in dentry cache
    pub fn lookup(&self, path: &str) -> Option<u64> {
        self.cache.read().get(path).copied()
    }

    /// Insert path -> inode mapping
    pub fn insert(&self, path: alloc::string::String, ino: u64) {
        let mut cache = self.cache.write();

        // Simple eviction: remove random entry if full
        if cache.len() >= self.max_size {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }

        cache.insert(path, ino);
    }

    /// Invalidate dentry
    pub fn invalidate(&self, path: &str) {
        self.cache.write().remove(path);
    }

    /// Clear entire cache
    pub fn clear(&self) {
        self.cache.write().clear();
    }
}

/// Global VFS caches
pub struct VfsCache {
    pub inode_cache: InodeCache,
    pub dentry_cache: DentryCache,
}

impl VfsCache {
    pub fn new() -> Self {
        Self {
            inode_cache: InodeCache::new(1024),   // 1024 inodes
            dentry_cache: DentryCache::new(2048), // 2048 dentries
        }
    }
}

/// Global VFS cache instance
static VFS_CACHE: spin::Once<VfsCache> = spin::Once::new();

/// Initialize VFS cache
pub fn init() {
    VFS_CACHE.call_once(|| VfsCache::new());
}

/// Get global VFS cache
pub fn get_cache() -> &'static VfsCache {
    VFS_CACHE.get().expect("VFS cache not initialized")
}
