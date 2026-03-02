// path/path_cache.rs — Dentry cache LRU ExoFS
// Ring 0, no_std
//
// Cache des résolutions de chemin (hash → ObjectId)
// Capacité : 10 000 entrées (règle PATH_CACHE_CAPACITY)
// Clé : hash FNV-1a du chemin absolu + root ObjectId

use crate::fs::exofs::core::{ObjectId, ExofsError, PATH_CACHE_CAPACITY};
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Statistiques ─────────────────────────────────────────────────────────────

/// Hits et misses du path cache
static PATH_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static PATH_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

// ─── Entrée de cache ──────────────────────────────────────────────────────────

struct PathCacheEntry {
    oid: ObjectId,
    /// Compteur d'accès pour LRU
    access_count: u64,
    /// Génération — invalidée au commit epoch
    generation: u64,
}

// ─── Cache global ─────────────────────────────────────────────────────────────

pub struct PathCache {
    inner: SpinLock<PathCacheInner>,
}

struct PathCacheInner {
    map: BTreeMap<u64, PathCacheEntry>,
    /// Compteur global d'accès (pour LRU)
    clock: u64,
    /// Génération courante (incrémentée à chaque commit epoch)
    generation: u64,
}

/// Instance globale du path cache
pub static GLOBAL_PATH_CACHE: PathCache = PathCache {
    inner: SpinLock::new(PathCacheInner {
        map: BTreeMap::new(),
        clock: 0,
        generation: 0,
    }),
};

impl PathCache {
    /// Lookup d'une entrée dans le cache
    /// Retourne None si absent ou si la génération ne correspond pas
    pub fn lookup(&self, hash: u64) -> Option<ObjectId> {
        let mut guard = self.inner.lock();
        let generation = guard.generation;
        if let Some(entry) = guard.map.get_mut(&hash) {
            if entry.generation == generation {
                entry.access_count = guard.clock;
                guard.clock += 1;
                PATH_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                return Some(entry.oid);
            }
        }
        PATH_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insère ou met à jour une entrée dans le cache
    pub fn insert(&self, hash: u64, oid: ObjectId) {
        let mut guard = self.inner.lock();
        let generation = guard.generation;

        // Si plein, évicte l'entrée LRU
        if guard.map.len() >= PATH_CACHE_CAPACITY {
            self.evict_lru_locked(&mut guard);
        }

        guard.clock += 1;
        let clock = guard.clock;
        guard.map.insert(hash, PathCacheEntry {
            oid,
            access_count: clock,
            generation,
        });
    }

    /// Invalide toutes les entrées (appelé au commit epoch)
    pub fn invalidate_all(&self) {
        let mut guard = self.inner.lock();
        guard.generation += 1;
        // Les entrées restent en map mais leur génération est périmée
    }

    /// Invalide une entrée spécifique par hash
    pub fn invalidate(&self, hash: u64) {
        let mut guard = self.inner.lock();
        guard.map.remove(&hash);
    }

    /// Statistiques du cache
    pub fn stats(&self) -> (u64, u64, usize) {
        let guard = self.inner.lock();
        (
            PATH_CACHE_HITS.load(Ordering::Relaxed),
            PATH_CACHE_MISSES.load(Ordering::Relaxed),
            guard.map.len(),
        )
    }

    /// Évicte l'entrée LRU (la moins récemment utilisée)
    fn evict_lru_locked(&self, guard: &mut PathCacheInner) {
        if guard.map.is_empty() {
            return;
        }
        // Trouve le minimum access_count
        let lru_key = guard.map.iter()
            .min_by_key(|(_, e)| e.access_count)
            .map(|(&k, _)| k);
        if let Some(k) = lru_key {
            guard.map.remove(&k);
        }
    }
}
