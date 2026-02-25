// kernel/src/fs/cache/eviction.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EVICTION — Shrinker & pression mémoire (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Lorsque le gestionnaire mémoire détecte une pression (low watermark), il appelle
// le shrinker FS pour libérer des pages, des dentries et des inodes non utilisés.
//
// Architecture :
//   • `ShrinkerTarget` : combien d'objets libérer de chaque caches.
//   • `run_shrinker(target)` : exécute un cycle de shrink coopératif.
//   • `ShrinkResult` : résumé de ce qui a été libéré.
//   • Les shrinkers FS ne font jamais d'I/O bloquant — si une page est dirty,
//     elle est ignorée (le writeback s'en occupe séparément).
//   • Lock ordering : on acquiert d'abord page_cache, puis inode_cache, puis dentry_cache.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::vec::Vec;

use crate::fs::core::types::FS_STATS;
use crate::fs::cache::page_cache::PAGE_CACHE;
use crate::fs::cache::inode_cache::INODE_HASH_CACHE;

// ─────────────────────────────────────────────────────────────────────────────
// Types publics
// ─────────────────────────────────────────────────────────────────────────────

/// Cible de shrinkage : nombre d'objets à libérer par type de cache.
#[derive(Clone, Copy, Debug)]
pub struct ShrinkerTarget {
    /// Pages LRU propres à libérer.
    pub pages:   usize,
    /// Inodes inutilisés à libérer.
    pub inodes:  usize,
    /// Dentries négatives / stales à libérer.
    pub dentries: usize,
    /// Buffers propres à libérer.
    pub buffers:  usize,
}

impl ShrinkerTarget {
    /// Pression légère (low watermark approché).
    pub const LIGHT: Self  = Self { pages: 64,  inodes: 16,  dentries: 32,  buffers: 16  };
    /// Pression modérée.
    pub const MEDIUM: Self = Self { pages: 256, inodes: 64,  dentries: 128, buffers: 64  };
    /// Pression forte (high watermark dépassé).
    pub const HEAVY: Self  = Self { pages: 1024, inodes: 256, dentries: 512, buffers: 256 };
}

/// Résultat d'un cycle de shrink.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShrinkResult {
    pub pages_freed:   usize,
    pub inodes_freed:  usize,
    pub dentries_freed: usize,
    pub buffers_freed:  usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Implémentation du shrinker
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute un cycle de shrinkage selon `target`.
///
/// Appelé par le gestionnaire mémoire (memory pressure callback).
pub fn run_shrinker(target: ShrinkerTarget) -> ShrinkResult {
    let mut result = ShrinkResult::default();
    EVICT_STATS.runs.fetch_add(1, Ordering::Relaxed);

    // 1. Libération des pages LRU propres.
    if target.pages > 0 {
        result.pages_freed = shrink_page_cache(target.pages);
    }

    // 2. Libération des inodes inutilisés.
    if target.inodes > 0 {
        result.inodes_freed = shrink_inode_cache(target.inodes);
    }

    // 3. Libération des dentries stales.
    if target.dentries > 0 {
        result.dentries_freed = shrink_dentry_cache(target.dentries);
    }

    EVICT_STATS.pages_freed.fetch_add(result.pages_freed as u64, Ordering::Relaxed);
    EVICT_STATS.inodes_freed.fetch_add(result.inodes_freed as u64, Ordering::Relaxed);
    EVICT_STATS.dentries_freed.fetch_add(result.dentries_freed as u64, Ordering::Relaxed);
    result
}

/// Libère jusqu'à `target` pages LRU propres du page cache.
fn shrink_page_cache(target: usize) -> usize {
    let pc = PAGE_CACHE.get();
    let mut freed = 0;

    'outer: for bucket in pc.buckets.iter() {
        let mut entries = bucket.entries.lock();
        let mut i = 0;
        while i < entries.len() && freed < target {
            let page = &entries[i].page;
            if page.is_evictable() && !page.dirty.load(Ordering::Relaxed) {
                entries.remove(i);
                pc.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.evictions.fetch_add(1, Ordering::Relaxed);
                freed += 1;
            } else {
                i += 1;
            }
        }
        if freed >= target { break 'outer; }
    }
    freed
}

/// Libère jusqu'à `target` inodes inutilisés.
fn shrink_inode_cache(target: usize) -> usize {
    let ic = INODE_HASH_CACHE.get();
    let mut freed = 0;

    'outer: for bucket in ic.buckets.iter() {
        let mut entries = bucket.entries.lock();
        let mut i = 0;
        while i < entries.len() && freed < target {
            let inode_ref = &entries[i].inode;
            // Un inode est libérable si uniquement le cache le référence (Arc::strong_count == 1).
            if alloc::sync::Arc::strong_count(inode_ref) == 1 {
                entries.remove(i);
                ic.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.inode_cache_count.fetch_sub(1, Ordering::Relaxed);
                freed += 1;
            } else {
                i += 1;
            }
        }
        if freed >= target { break 'outer; }
    }
    freed
}

/// Libère jusqu'à `target` dentries négatives ou sans inode actif.
fn shrink_dentry_cache(target: usize) -> usize {
    use crate::fs::core::dentry::DENTRY_CACHE;
    let dc = &DENTRY_CACHE;
    let mut freed = 0;

    'outer: for bucket in dc.buckets.iter() {
        let mut entries = bucket.entries.lock();
        let mut i = 0;
        while i < entries.len() && freed < target {
            let dref = &entries[i];
            let is_negative = {
                let d = dref.dentry.read();
                d.inode.is_none()
                    && alloc::sync::Arc::strong_count(&dref.dentry) == 2
            };
            if is_negative {
                entries.remove(i);
                dc.total.fetch_sub(1, Ordering::Relaxed);
                FS_STATS.dentry_cache_count.fetch_sub(1, Ordering::Relaxed);
                freed += 1;
            } else {
                i += 1;
            }
        }
        if freed >= target { break 'outer; }
    }
    freed
}

// ─────────────────────────────────────────────────────────────────────────────
// Critères de shrink : mémoire libre estimée
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le niveau de pression actuel d'après les stats du page cache.
pub fn estimate_pressure() -> ShrinkerTarget {
    let total   = FS_STATS.page_cache_pages.load(Ordering::Relaxed);
    let dirty   = FS_STATS.dirty_pages.load(Ordering::Relaxed);
    let clean   = total.saturating_sub(dirty);

    if clean > 4096 { ShrinkerTarget::LIGHT }
    else if clean > 1024 { ShrinkerTarget::MEDIUM }
    else { ShrinkerTarget::HEAVY }
}

// ─────────────────────────────────────────────────────────────────────────────
// EvictionStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct EvictionStats {
    pub runs:           AtomicU64,
    pub pages_freed:    AtomicU64,
    pub inodes_freed:   AtomicU64,
    pub dentries_freed: AtomicU64,
    pub buffers_freed:  AtomicU64,
}

impl EvictionStats {
    pub const fn new() -> Self {
        Self {
            runs:           AtomicU64::new(0),
            pages_freed:    AtomicU64::new(0),
            inodes_freed:   AtomicU64::new(0),
            dentries_freed: AtomicU64::new(0),
            buffers_freed:  AtomicU64::new(0),
        }
    }
}

pub static EVICT_STATS: EvictionStats = EvictionStats::new();
