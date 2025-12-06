//! Page Cache for Filesystem I/O
//!
//! Provides buffering and caching for block device I/O operations.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use lazy_static::lazy_static;

/// Page size (4KB)
const PAGE_SIZE: usize = 4096;

/// Maximum cache size (in pages)
const MAX_CACHE_PAGES: usize = 1024; // 4MB cache

/// Cached page
struct CachedPage {
    /// Page data
    data: Vec<u8>,
    /// Access count (for LRU)
    access_count: u64,
    /// Last access time (approximate)
    last_access: u64,
    /// Dirty flag
    dirty: bool,
}

/// Page cache key (device + block)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PageKey {
    device_id: u64,
    block: u64,
}

/// Global page cache
lazy_static! {
    static ref PAGE_CACHE: Mutex<PageCache> = Mutex::new(PageCache::new());
}

/// Page Cache
pub struct PageCache {
    /// Cached pages
    pages: BTreeMap<PageKey, CachedPage>,
    /// Access counter for LRU
    access_counter: u64,
}

impl PageCache {
    pub fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
            access_counter: 0,
        }
    }
    
    /// Get a page from cache
    pub fn get(&mut self, device_id: u64, block: u64) -> Option<Vec<u8>> {
        let key = PageKey { device_id, block };
        
        if let Some(page) = self.pages.get_mut(&key) {
            self.access_counter += 1;
            page.access_count += 1;
            page.last_access = self.access_counter;
            
            return Some(page.data.clone());
        }
        
        None
    }
    
    /// Put a page into cache
    pub fn put(&mut self, device_id: u64, block: u64, data: Vec<u8>) {
        // Evict if cache is full
        if self.pages.len() >= MAX_CACHE_PAGES {
            self.evict_lru();
        }
        
        let key = PageKey { device_id, block };
        
        self.access_counter += 1;
        
        let page = CachedPage {
            data,
            access_count: 1,
            last_access: self.access_counter,
            dirty: false,
        };
        
        self.pages.insert(key, page);
    }
    
    /// Mark page as dirty
    pub fn mark_dirty(&mut self, device_id: u64, block: u64) {
        let key = PageKey { device_id, block };
        
        if let Some(page) = self.pages.get_mut(&key) {
            page.dirty = true;
        }
    }
    
    /// Evict least recently used page
    fn evict_lru(&mut self) {
        if self.pages.is_empty() {
            return;
        }
        
        // Find LRU page
        let mut lru_key = None;
        let mut lru_access = u64::MAX;
        
        for (key, page) in &self.pages {
            if page.last_access < lru_access {
                lru_access = page.last_access;
                lru_key = Some(*key);
            }
        }
        
        if let Some(key) = lru_key {
            // Write back if dirty
            if let Some(page) = self.pages.get(&key) {
                if page.dirty {
                    // Appeler write_page_to_device
                    self.write_page_to_device(key, &page.data);
                }
            }
            self.pages.remove(&key);
        }
    }
    
    /// Flush all dirty pages
    pub fn flush_all(&mut self) {
        // Write back all dirty pages
        let keys: Vec<_> = self.pages.iter()
            .filter(|(_, page)| page.dirty)
            .map(|(key, _)| *key)
            .collect();
        
        for key in keys {
            if let Some(page) = self.pages.get_mut(&key) {
                if page.dirty {
                    self.write_page_to_device(key, &page.data);
                    page.dirty = false;
                }
            }
        }
    }
    
    /// Write page to device (stub - devrait utiliser BlockDevice trait)
    fn write_page_to_device(&self, key: PageKey, data: &[u8]) {
        // Dans une vraie implémentation:
        // 1. Obtenir BlockDevice via device_id
        // 2. Appeler device.write(key.block, data)
        // Pour l'instant, on simule juste le write-back
        log::trace!("cache: write_back device={} block={} size={}", 
                    key.device_id, key.block, data.len());
        // Note: L'intégration complète nécessite un registry de BlockDevice
        // et l'accès au device depuis le cache, ce qui nécessiterait
        // une refonte de l'architecture (device registry global).
    }
    
    /// Clear cache
    pub fn clear(&mut self) {
        self.flush_all();
        self.pages.clear();
        self.access_counter = 0;
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> (usize, usize) {
        let total = self.pages.len();
        let dirty = self.pages.values().filter(|p| p.dirty).count();
        (total, dirty)
    }
}

/// Get a cached page
pub fn get_cached_page(device_id: u64, block: u64) -> Option<Vec<u8>> {
    PAGE_CACHE.lock().get(device_id, block)
}

/// Cache a page
pub fn cache_page(device_id: u64, block: u64, data: Vec<u8>) {
    PAGE_CACHE.lock().put(device_id, block, data);
}

/// Mark page as dirty
pub fn mark_page_dirty(device_id: u64, block: u64) {
    PAGE_CACHE.lock().mark_dirty(device_id, block);
}

/// Flush all cached pages
pub fn flush_cache() {
    PAGE_CACHE.lock().flush_all();
}

/// Clear the cache
pub fn clear_cache() {
    PAGE_CACHE.lock().clear();
}

/// Get cache statistics
pub fn cache_stats() -> (usize, usize) {
    PAGE_CACHE.lock().stats()
}

/// Initialize page cache
pub fn init() {
    log::info!("Page cache initialized (max {} pages = {}KB)",
               MAX_CACHE_PAGES, MAX_CACHE_PAGES * PAGE_SIZE / 1024);
}
