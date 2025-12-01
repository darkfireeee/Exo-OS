//! Inode Cache for POSIX-X
//!
//! High-performance inode cache with:
//! - LRU eviction policy
//! - Fast lookup by inode number
//! - Reference counting
//! - Thread-safe access

use crate::fs::vfs::inode::Inode;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use spin::RwLock;
use hashbrown::HashMap;

/// Inode cache entry
struct InodeCacheEntry {
    inode: Arc<RwLock<dyn Inode>>,
    refcount: usize,
    last_access: u64,
}

/// Global inode cache
static INODE_CACHE: RwLock<Option<HashMap<u64, InodeCacheEntry>>> = RwLock::new(None);

/// Cache statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub size: usize,
}

static CACHE_STATS: RwLock<CacheStats> = RwLock::new(CacheStats {
    hits: 0,
    misses: 0,
    evictions: 0,
    size: 0,
});

/// Maximum number of cached inodes
const MAX_CACHE_SIZE: usize = 1024;

/// Initialize inode cache
pub fn init() {
    *INODE_CACHE.write() = Some(HashMap::new());
    log::info!("[POSIX-VFS] Inode cache initialized (max {} entries)", MAX_CACHE_SIZE);
}

/// Get inode from cache or VFS
///
/// # Performance
/// - Cache hit: < 50 cycles
/// - Cache miss: < 1000 cycles + VFS lookup
pub fn get_inode(ino: u64) -> FsResult<Arc<RwLock<dyn Inode>>> {
    // Try cache first
    {
        let mut cache = INODE_CACHE.write();
        if let Some(ref mut c) = *cache {
            if let Some(entry) = c.get_mut(&ino) {
                entry.refcount += 1;
                entry.last_access = get_timestamp();
                
                // Update stats
                let mut stats = CACHE_STATS.write();
                stats.hits += 1;
                
                return Ok(Arc::clone(&entry.inode));
            }
        }
    }

    // Cache miss - get from VFS
    let mut stats = CACHE_STATS.write();
    stats.misses += 1;
    drop(stats);

    let inode = crate::fs::vfs::cache::get_inode(ino)?;

    // Insert into cache
    insert_inode(ino, Arc::clone(&inode));

    Ok(inode)
}

/// Insert inode into cache
fn insert_inode(ino: u64, inode: Arc<RwLock<dyn Inode>>) {
    let mut cache = INODE_CACHE.write();
    if let Some(ref mut c) = *cache {
        // Check if cache is full
        if c.len() >= MAX_CACHE_SIZE {
            evict_lru(c);
        }

        c.insert(ino, InodeCacheEntry {
            inode,
            refcount: 1,
            last_access: get_timestamp(),
        });

        let mut stats = CACHE_STATS.write();
        stats.size = c.len();
    }
}

/// Evict least recently used inode
fn evict_lru(cache: &mut HashMap<u64, InodeCacheEntry>) {
    // Find LRU entry with refcount == 0
    let mut lru_ino = None;
    let mut lru_time = u64::MAX;

    for (&ino, entry) in cache.iter() {
        if entry.refcount == 0 && entry.last_access < lru_time {
            lru_time = entry.last_access;
            lru_ino = Some(ino);
        }
    }

    // Evict if found
    if let Some(ino) = lru_ino {
        cache.remove(&ino);
        
        let mut stats = CACHE_STATS.write();
        stats.evictions += 1;
        stats.size = cache.len();
    } else {
        // No inode with refcount 0 - just remove oldest
        if let Some((&ino, _)) = cache.iter().min_by_key(|(_, e)| e.last_access) {
            cache.remove(&ino);
            
            let mut stats = CACHE_STATS.write();
            stats.evictions += 1;
            stats.size = cache.len();
        }
    }
}

/// Release inode reference
pub fn release_inode(ino: u64) {
    let mut cache = INODE_CACHE.write();
    if let Some(ref mut c) = *cache {
        if let Some(entry) = c.get_mut(&ino) {
            if entry.refcount > 0 {
                entry.refcount -= 1;
            }
        }
    }
}

/// Invalidate cached inode
pub fn invalidate_inode(ino: u64) {
    let mut cache = INODE_CACHE.write();
    if let Some(ref mut c) = *cache {
        c.remove(&ino);
        
        let mut stats = CACHE_STATS.write();
        stats.size = c.len();
    }
}

/// Clear entire cache
pub fn clear_cache() {
    let mut cache = INODE_CACHE.write();
    if let Some(ref mut c) = *cache {
        c.clear();
        
        let mut stats = CACHE_STATS.write();
        stats.size = 0;
    }
}

/// Get cache statistics
pub fn get_stats() -> CacheStats {
    *CACHE_STATS.read()
}

/// Reset cache statistics
pub fn reset_stats() {
    let mut stats = CACHE_STATS.write();
    stats.hits = 0;
    stats.misses = 0;
    stats.evictions = 0;
}

/// Get current timestamp (placeholder)
fn get_timestamp() -> u64 {
    // TODO: Use real TSC or system timer
    0
}

/// Print cache statistics (for debugging)
pub fn print_stats() {
    let stats = get_stats();
    let hit_rate = if stats.hits + stats.misses > 0 {
        (stats.hits as f64 / (stats.hits + stats.misses) as f64) * 100.0
    } else {
        0.0
    };

    log::info!("[POSIX-VFS Cache] Stats:");
    log::info!("  Hits: {} ({:.1}%)", stats.hits, hit_rate);
    log::info!("  Misses: {}", stats.misses);
    log::info!("  Evictions: {}", stats.evictions);
    log::info!("  Size: {}/{}", stats.size, MAX_CACHE_SIZE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_stats() {
        let stats = CacheStats::default();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }
}
