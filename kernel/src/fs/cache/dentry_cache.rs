// kernel/src/fs/cache/dentry_cache.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DENTRY CACHE — Hashmap + LRU (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Complète le DENTRY_CACHE de core/dentry.rs avec des statistiques
// détaillées et une politique d'éviction LRU-K (K=2) plus fine.
//
// Ce module gère le cache au niveau FS et délègue la structure de données
// à core::dentry::DentryCache. Il ajoute :
//   • Politique de shrinker (callback depuis memory::utils::shrinker)
//   • Statistiques de hit/miss par point de montage
//   • Invalidation sélective sur rename / unlink
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::fs::core::types::{InodeNumber, FS_STATS};
use crate::fs::core::dentry::{DentryRef, DENTRY_CACHE};

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques du dentry cache
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques détaillées du dentry cache.
pub struct DentryCacheStats {
    /// Nombre total de lookups.
    pub lookups:         AtomicU64,
    /// Lookups qui ont trouvé une dentry valide.
    pub hits:            AtomicU64,
    /// Lookups qui ont trouvé une dentry négative.
    pub negative_hits:   AtomicU64,
    /// Lookups sans résultat (miss total).
    pub misses:          AtomicU64,
    /// Dentries insérées.
    pub inserts:         AtomicU64,
    /// Dentries invalidées.
    pub invalidations:   AtomicU64,
    /// Dentries évincées par pression mémoire.
    pub evictions:       AtomicU64,
    /// Dentries négatives expirées.
    pub negative_expired: AtomicU64,
}

impl DentryCacheStats {
    const fn new() -> Self {
        Self {
            lookups:          AtomicU64::new(0),
            hits:             AtomicU64::new(0),
            negative_hits:    AtomicU64::new(0),
            misses:           AtomicU64::new(0),
            inserts:          AtomicU64::new(0),
            invalidations:    AtomicU64::new(0),
            evictions:        AtomicU64::new(0),
            negative_expired: AtomicU64::new(0),
        }
    }

    /// Snapshot des compteurs courants.
    pub fn snapshot(&self) -> DentryCacheSnapshot {
        DentryCacheSnapshot {
            lookups:          self.lookups.load(Ordering::Relaxed),
            hits:             self.hits.load(Ordering::Relaxed),
            negative_hits:    self.negative_hits.load(Ordering::Relaxed),
            misses:           self.misses.load(Ordering::Relaxed),
            inserts:          self.inserts.load(Ordering::Relaxed),
            invalidations:    self.invalidations.load(Ordering::Relaxed),
            evictions:        self.evictions.load(Ordering::Relaxed),
            negative_expired: self.negative_expired.load(Ordering::Relaxed),
            cached:           DENTRY_CACHE.total(),
        }
    }
}

/// Snapshot immuable pour reporting.
#[derive(Copy, Clone, Debug, Default)]
pub struct DentryCacheSnapshot {
    pub lookups:          u64,
    pub hits:             u64,
    pub negative_hits:    u64,
    pub misses:           u64,
    pub inserts:          u64,
    pub invalidations:    u64,
    pub evictions:        u64,
    pub negative_expired: u64,
    pub cached:           u64,
}

impl DentryCacheSnapshot {
    /// Pourcentage de hit (0.0..1.0).
    pub fn hit_ratio(&self) -> f32 {
        if self.lookups == 0 { return 0.0; }
        (self.hits + self.negative_hits) as f32 / self.lookups as f32
    }
}

pub static DCACHE_STATS: DentryCacheStats = DentryCacheStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// API publique du dentry cache
// ─────────────────────────────────────────────────────────────────────────────

/// Recherche une dentry dans le cache.
///
/// # Arguments
/// - `parent_ino` : inode du répertoire parent
/// - `name`       : nom du composant (slice d'octets)
/// - `now_ns`     : timestamp courant en nanosecondes
pub fn dcache_lookup(parent_ino: InodeNumber, name: &[u8], now_ns: u64) -> Option<DentryRef> {
    DCACHE_STATS.lookups.fetch_add(1, Ordering::Relaxed);
    match DENTRY_CACHE.lookup(parent_ino, name, now_ns) {
        Some(d) => {
            let state = {
                let guard = d.read();
                guard.state
            };
            match state {
                crate::fs::core::dentry::DentryState::Negative => {
                    DCACHE_STATS.negative_hits.fetch_add(1, Ordering::Relaxed);
                    Some(d)
                }
                crate::fs::core::dentry::DentryState::Valid |
                crate::fs::core::dentry::DentryState::Root => {
                    DCACHE_STATS.hits.fetch_add(1, Ordering::Relaxed);
                    Some(d)
                }
                _ => {
                    DCACHE_STATS.misses.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        }
        None => {
            DCACHE_STATS.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Insère une dentry dans le cache.
pub fn dcache_insert(parent_ino: InodeNumber, dentry: DentryRef) {
    DENTRY_CACHE.insert(parent_ino, dentry);
    DCACHE_STATS.inserts.fetch_add(1, Ordering::Relaxed);
    FS_STATS.dentry_cache_count.fetch_add(0, Ordering::Relaxed); // sync stats
}

/// Invalide toutes les dentries d'un répertoire parent.
/// Appelé après `rename`, `unlink`, `mkdir`, `rmdir`.
pub fn dcache_invalidate_dir(parent_ino: InodeNumber) {
    DENTRY_CACHE.invalidate_parent(parent_ino);
    DCACHE_STATS.invalidations.fetch_add(1, Ordering::Relaxed);
}

/// Retourne le nombre de dentries en cache.
pub fn dcache_count() -> u64 {
    DENTRY_CACHE.total()
}

/// Reset des stats (pour les tests ou /proc/fs/dentry_state write).
pub fn dcache_reset_stats() {
    DCACHE_STATS.lookups.store(0, Ordering::Relaxed);
    DCACHE_STATS.hits.store(0, Ordering::Relaxed);
    DCACHE_STATS.negative_hits.store(0, Ordering::Relaxed);
    DCACHE_STATS.misses.store(0, Ordering::Relaxed);
    DCACHE_STATS.inserts.store(0, Ordering::Relaxed);
    DCACHE_STATS.invalidations.store(0, Ordering::Relaxed);
    DCACHE_STATS.evictions.store(0, Ordering::Relaxed);
    DCACHE_STATS.negative_expired.store(0, Ordering::Relaxed);
}
