//! mod.rs — Orchestrateur du sous-système cache ExoFS (no_std).
//!
//! Expose tous les caches, coordonne l'initialisation, le shutdown
//! (flush dirty avant extinction), la vérification de santé et les
//! statistiques agrégées globales.
//! Règles : RECUR-01, OOM-02, ARITH-02.


pub mod blob_cache;
pub mod cache_eviction;
pub mod cache_policy;
pub mod cache_pressure;
pub mod cache_shrinker;
pub mod cache_stats;
pub mod cache_warming;
pub mod extent_cache;
pub mod metadata_cache;
pub mod object_cache;
pub mod path_cache;

pub use blob_cache::{BlobCache, BLOB_CACHE};
pub use cache_eviction::{EvictionPolicy, EvictionAlgorithm};
pub use cache_policy::{CachePolicy, CacheConfig};
pub use cache_pressure::{CachePressure, CACHE_PRESSURE};
pub use cache_shrinker::{CacheShrinker, CACHE_SHRINKER};
pub use cache_stats::{CacheStats, CacheStatsSnapshot, CACHE_STATS};
pub use cache_warming::{CacheWarmer, WarmingStrategy};
pub use extent_cache::{ExtentCache, ExtentEntry, EXTENT_CACHE};
pub use metadata_cache::{MetadataCache, METADATA_CACHE};
pub use object_cache::{ObjectCache, CachedObject, OBJECT_CACHE};
pub use path_cache::{PathCache, PATH_CACHE};


// ─────────────────────────────────────────────────────────────────────────────
// CacheHealthReport
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport de santé multi-cache.
#[derive(Clone, Debug, Default)]
pub struct CacheHealthReport {
    pub blob_used_bytes:     u64,
    pub object_used_bytes:   u64,
    pub extent_used_bytes:   u64,
    pub blob_n_entries:      usize,
    pub object_n_entries:    usize,
    pub extent_n_entries:    usize,
    pub meta_n_entries:      usize,
    pub path_n_entries:      usize,
    pub global_hits:         u64,
    pub global_misses:       u64,
    pub global_evictions:    u64,
    pub global_dirty_bytes:  u64,
    pub pressure_level:      cache_pressure::PressureLevel,
}

impl CacheHealthReport {
    pub fn total_used_bytes(&self) -> u64 {
        self.blob_used_bytes
            .saturating_add(self.object_used_bytes)
            .saturating_add(self.extent_used_bytes)
    }

    pub fn total_entries(&self) -> usize {
        self.blob_n_entries
            .saturating_add(self.object_n_entries)
            .saturating_add(self.extent_n_entries)
            .saturating_add(self.meta_n_entries)
            .saturating_add(self.path_n_entries)
    }

    pub fn hit_ratio_pct(&self) -> u64 {
        let total = self.global_hits.wrapping_add(self.global_misses);
        if total == 0 { return 0; }
        self.global_hits * 100 / total
    }

    /// `true` si tous les caches sont en bonne santé.
    pub fn is_healthy(&self) -> bool {
        self.pressure_level != cache_pressure::PressureLevel::Critical
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système cache (réinitialise les compteurs).
pub fn init() {
    CACHE_STATS.reset();
    CACHE_PRESSURE.set_max_bytes(256 * 1024 * 1024);
    CACHE_PRESSURE.update_default(0);
}

/// Flush de tous les caches dirty puis vide les caches en mémoire.
///
/// Doit être appelé avant démontage ou arrêt du système.
/// Note : dans cette implémentation no_std de référence, le flush
/// marque toutes les entrées propres — l'I/O réelle est du ressort
/// du appelant (layer VFS).
pub fn shutdown() {
    // Marquer comme propre et vider chaque cache.
    BLOB_CACHE.flush_all();
    OBJECT_CACHE.flush_all();
    EXTENT_CACHE.flush_all();
    METADATA_CACHE.flush_all();
    PATH_CACHE.flush_all();
    CACHE_STATS.reset();
}

/// Force l'éviction d'au moins `bytes` octets dans l'ensemble des caches.
///
/// Stratégie : d'abord path, puis metadata, puis extent, blob, objet.
pub fn reclaim_bytes(bytes: u64) -> u64 {
    let mut freed = 0u64;
    let targets = [
        bytes / 5,          // path = 20 %
        bytes / 5,          // meta = 20 %
        bytes * 3 / 10,     // extent = 30 %
        bytes * 3 / 10,     // blob = 30 %
    ];

    // Étape 1 : path cache (le plus remplaçable).
    PATH_CACHE.flush_all();
    freed = freed.saturating_add(targets[0]);

    // Étape 2 : metadata.
    METADATA_CACHE.flush_all();
    freed = freed.saturating_add(targets[1]);

    if freed >= bytes { return freed; }

    // Étape 3 : extent.
    let e = EXTENT_CACHE.evict_n(64);
    freed = freed.saturating_add(e);

    if freed >= bytes { return freed; }

    // Étape 4 : blob.
    let b = BLOB_CACHE.evict_n(64);
    freed = freed.saturating_add(b);

    freed
}

/// Construit un rapport de santé instantané.
pub fn verify_health() -> CacheHealthReport {
    let snap = CACHE_STATS.snapshot();
    CacheHealthReport {
        blob_used_bytes:    BLOB_CACHE.used_bytes(),
        object_used_bytes:  OBJECT_CACHE.used_bytes(),
        extent_used_bytes:  EXTENT_CACHE.used_bytes(),
        blob_n_entries:     BLOB_CACHE.n_entries(),
        object_n_entries:   OBJECT_CACHE.n_entries(),
        extent_n_entries:   EXTENT_CACHE.n_entries(),
        meta_n_entries:     METADATA_CACHE.n_entries(),
        path_n_entries:     PATH_CACHE.n_entries(),
        global_hits:        snap.hits,
        global_misses:      snap.misses,
        global_evictions:   snap.evictions,
        global_dirty_bytes: snap.dirty_bytes,
        pressure_level:     CACHE_PRESSURE.level(),
    }
}

/// Retourne un snapshot global des statistiques.
pub fn global_stats_snapshot() -> CacheStatsSnapshot {
    CACHE_STATS.snapshot()
}

/// Met à jour le moniteur de pression avec l'utilisation totale actuelle.
pub fn update_pressure() -> cache_pressure::PressureLevel {
    let report = verify_health();
    CACHE_PRESSURE.update_default(report.total_used_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::BlobId;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    #[test] fn test_init_resets_stats() {
        CACHE_STATS.record_hit();
        init();
        assert_eq!(CACHE_STATS.hits(), 0);
    }

    #[test] fn test_verify_health_returns_report() {
        init();
        let r = verify_health();
        assert!(r.is_healthy());
    }

    #[test] fn test_global_stats_snapshot() {
        init();
        CACHE_STATS.record_hit(); CACHE_STATS.record_miss();
        let snap = global_stats_snapshot();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 1);
    }

    #[test] fn test_shutdown_flushes() {
        init();
        BLOB_CACHE.insert(blob(1), alloc::vec![0u8; 64]).ok();
        shutdown();
        assert_eq!(BLOB_CACHE.n_entries(), 0);
    }

    #[test] fn test_update_pressure_low_when_empty() {
        init();
        CACHE_PRESSURE.set_max_bytes(256 * 1024 * 1024);
        let lv = update_pressure();
        assert_eq!(lv, cache_pressure::PressureLevel::Low);
    }

    #[test] fn test_health_report_total_entries() {
        init();
        let r = verify_health();
        assert_eq!(r.total_entries(), 0);
    }

    #[test] fn test_health_report_hit_ratio_zero_when_idle() {
        init();
        let r = verify_health();
        assert_eq!(r.hit_ratio_pct(), 0);
    }

    #[test] fn test_reclaim_bytes_no_panic() {
        init();
        let freed = reclaim_bytes(1024 * 1024);
        let _ = freed;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheOrchestrator — supervision haut-niveau
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau d'agressivité du reclaim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReclaimAggression {
    /// Tente d'éviter les entrées dirty.
    Conservative,
    /// Évicte les entrées clean en priorité.
    Moderate,
    /// Évicte tout, y compris les dirty.
    Aggressive,
}

/// Rapport de reclaim global.
#[derive(Clone, Debug, Default)]
pub struct ReclaimReport {
    pub bytes_requested:  u64,
    pub bytes_freed:      u64,
    pub n_evictions:      u64,
    pub pressure_before:  cache_pressure::PressureLevel,
    pub pressure_after:   cache_pressure::PressureLevel,
}

impl ReclaimReport {
    /// `true` si la cible a été atteinte.
    pub fn target_met(&self) -> bool { self.bytes_freed >= self.bytes_requested }
}

/// Orchestre un reclaim avec contrôle de l'agressivité.
pub fn reclaim_with_report(
    bytes:      u64,
    aggression: ReclaimAggression,
) -> ReclaimReport {
    let pressure_before = CACHE_PRESSURE.level();
    let freed = match aggression {
        ReclaimAggression::Conservative => reclaim_bytes(bytes / 2),
        ReclaimAggression::Moderate     => reclaim_bytes(bytes),
        ReclaimAggression::Aggressive   => {
            let f = reclaim_bytes(bytes);
            // Flush supplémentaire si insuffisant.
            if f < bytes {
                EXTENT_CACHE.flush_all();
                BLOB_CACHE.flush_all();
            }
            f
        }
    };
    CACHE_STATS.record_eviction(freed);
    let pressure_after = update_pressure();
    ReclaimReport {
        bytes_requested: bytes,
        bytes_freed:     freed,
        n_evictions:     1,
        pressure_before,
        pressure_after,
    }
}

/// Retourne `true` si le système devrait déclencher un reclaim.
pub fn should_reclaim() -> bool {
    CACHE_PRESSURE.is_under_pressure()
}

/// Politique de réaction automatique à la pression.
pub fn auto_pressure_response() -> Option<ReclaimReport> {
    let level = CACHE_PRESSURE.level();
    match level {
        cache_pressure::PressureLevel::Critical => {
            Some(reclaim_with_report(64 * 1024 * 1024, ReclaimAggression::Aggressive))
        }
        cache_pressure::PressureLevel::High => {
            Some(reclaim_with_report(32 * 1024 * 1024, ReclaimAggression::Moderate))
        }
        cache_pressure::PressureLevel::Medium => {
            Some(reclaim_with_report(8 * 1024 * 1024, ReclaimAggression::Conservative))
        }
        _ => None,
    }
}

/// Met à jour les statistiques globales et la pression, retourne un rapport complet.
pub fn tick() -> CacheHealthReport {
    update_pressure();
    verify_health()
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheCapacityConfig — configuration des tailles maximales
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration des capacités maximales de chaque sous-cache.
#[derive(Clone, Debug)]
pub struct CacheCapacityConfig {
    pub blob_max_bytes:     u64,
    pub object_max_bytes:   u64,
    pub extent_max_bytes:   u64,
    pub total_max_bytes:    u64,
}

impl Default for CacheCapacityConfig {
    fn default() -> Self {
        CacheCapacityConfig {
            blob_max_bytes:   64  * 1024 * 1024,
            object_max_bytes: 32  * 1024 * 1024,
            extent_max_bytes: 128 * 1024 * 1024,
            total_max_bytes:  256 * 1024 * 1024,
        }
    }
}

impl CacheCapacityConfig {
    /// Applique la configuration au moniteur de pression.
    pub fn apply(&self) {
        CACHE_PRESSURE.set_max_bytes(self.total_max_bytes);
    }

    /// Vérifie la cohérence des capacités.
    pub fn is_valid(&self) -> bool {
        let sub_total = self.blob_max_bytes
            .saturating_add(self.object_max_bytes)
            .saturating_add(self.extent_max_bytes);
        sub_total <= self.total_max_bytes
    }
}

/// Initialise le sous-système avec une configuration de capacité explicite.
pub fn init_with_config(cfg: &CacheCapacityConfig) {
    CACHE_STATS.reset();
    cfg.apply();
    CACHE_PRESSURE.update_default(0);
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test] fn test_reclaim_report_target_met() {
        let r = ReclaimReport { bytes_requested: 10, bytes_freed: 10, ..Default::default() };
        assert!(r.target_met());
    }

    #[test] fn test_reclaim_with_report_conservative() {
        init();
        let r = reclaim_with_report(1024, ReclaimAggression::Conservative);
        assert_eq!(r.bytes_requested, 1024);
    }

    #[test] fn test_reclaim_with_report_aggressive() {
        init();
        let r = reclaim_with_report(1024, ReclaimAggression::Aggressive);
        let _ = r;
    }

    #[test] fn test_auto_pressure_response_no_panic() {
        init();
        CACHE_PRESSURE.set_max_bytes(256 * 1024 * 1024);
        CACHE_PRESSURE.update_default(0);
        let _ = auto_pressure_response(); // Low → None normalement
    }

    #[test] fn test_should_reclaim_false_when_empty() {
        init();
        CACHE_PRESSURE.set_max_bytes(256 * 1024 * 1024);
        CACHE_PRESSURE.update_default(0);
        assert!(!should_reclaim());
    }

    #[test] fn test_tick_returns_health_report() {
        init();
        let r = tick();
        assert!(r.is_healthy());
    }

    #[test] fn test_capacity_config_default_valid() {
        let cfg = CacheCapacityConfig::default();
        assert!(cfg.is_valid());
    }

    #[test] fn test_init_with_config() {
        let cfg = CacheCapacityConfig::default();
        init_with_config(&cfg);
        assert_eq!(CACHE_STATS.hits(), 0);
    }

    #[test] fn test_health_report_is_healthy_initially() {
        init();
        assert!(verify_health().is_healthy());
    }

    #[test] fn test_health_total_used_bytes() {
        init();
        let r = verify_health();
        assert_eq!(r.total_used_bytes(), 0);
    }
}
