//! Cache - Multi-tier intelligent caching system
//!
//! ## Modules
//! - `page_cache`: Page-level caching
//! - `inode_cache`: Inode metadata caching
//! - `buffer`: Block-level buffer cache
//! - `eviction`: Advanced eviction policies
//! - `prefetch`: Intelligent read-ahead with pattern detection
//! - `tiering`: Hot/Warm/Cold data classification
//!
//! ## Performance Targets
//! - Page cache hit rate: > 95%
//! - Inode cache hit rate: > 98%
//! - Buffer cache hit rate: > 90%
//! - Prefetch accuracy: > 80%

pub mod page_cache;
pub mod inode_cache;
pub mod buffer;
pub mod eviction;
pub mod prefetch;
pub mod tiering;

use crate::fs::FsResult;

/// Initialize all cache subsystems
pub fn init(page_cache_mb: usize, max_inodes: usize, max_buffers: usize) {
    log::info!("Initializing cache subsystem");
    log::info!("  Page cache: {} MB", page_cache_mb);
    log::info!("  Inode cache: {} entries", max_inodes);
    log::info!("  Buffer cache: {} buffers ({} MB)", max_buffers, (max_buffers * buffer::BLOCK_SIZE) / (1024 * 1024));

    // Calculate page cache size
    let max_pages = (page_cache_mb * 1024 * 1024) / page_cache::PAGE_SIZE;

    // Initialize each cache
    page_cache::init(max_pages);
    inode_cache::init(max_inodes);
    buffer::init(max_buffers);

    // Initialize intelligent subsystems
    prefetch::init();
    tiering::init();

    log::info!("✓ Cache subsystem initialized with prefetch and tiering");
}

/// Get aggregate cache statistics
pub fn get_stats() -> CacheStats {
    CacheStats {
        page_cache_hit_rate: page_cache::global_page_cache().hit_rate(),
        page_cache_size: page_cache::global_page_cache().len(),
        inode_cache_hit_rate: inode_cache::global_inode_cache().hit_rate(),
        inode_cache_size: inode_cache::global_inode_cache().len(),
        buffer_cache_hit_rate: buffer::global_buffer_cache().hit_rate(),
        buffer_cache_size: buffer::global_buffer_cache().len(),
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub page_cache_hit_rate: f64,
    pub page_cache_size: usize,
    pub inode_cache_hit_rate: f64,
    pub inode_cache_size: usize,
    pub buffer_cache_hit_rate: f64,
    pub buffer_cache_size: usize,
}

/// Sync all caches
pub fn sync_all() -> FsResult<()> {
    log::debug!("Syncing all caches");

    page_cache::global_page_cache().sync_all()?;
    buffer::global_buffer_cache().sync_all()?;

    log::debug!("✓ All caches synced");
    Ok(())
}
