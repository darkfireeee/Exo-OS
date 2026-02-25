// kernel/src/fs/cache/mod.rs
//
// Cache FS — page cache, inode cache, dentry cache, buffer cache,
// prefetch adaptatif, éviction sous pression mémoire.

pub mod page_cache;
pub mod inode_cache;
pub mod dentry_cache;
pub mod buffer;
pub mod prefetch;
pub mod eviction;
/// Thread de writeback + delayed allocation (RÈGLE FS-EXT4P-03).
pub mod writeback;

pub use page_cache::{
    CachedPage, PageIndex, PageRef, PAGE_CACHE, GlobalPageCache,
    page_cache_init,
};
pub use inode_cache::{
    INODE_HASH_CACHE, GlobalInodeCache,
    inode_cache_init,
};
pub use dentry_cache::{
    DCACHE_STATS, DentryCacheSnapshot,
    dcache_lookup, dcache_insert, dcache_invalidate_dir,
    dcache_count, dcache_reset_stats,
};
pub use buffer::{
    BufHead, BufRef, BlockNumber, BufferCache, BUFFER_CACHE,
    buffer_cache_init, BUF_SIZE,
};
pub use prefetch::{
    ReadaheadState, RA_STATS,
    prefetch_pages, maybe_prefetch,
};
pub use eviction::{
    ShrinkerTarget, ShrinkResult, EVICT_STATS,
    run_shrinker, estimate_pressure,
};
pub use writeback::{
    WRITEBACK_STATS, WritebackStats,
    writeback_run_once, writeback_under_pressure, writeback_thread_loop,
    WRITEBACK_INTERVAL_MS,
};
