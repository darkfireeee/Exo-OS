//! cache_policy.rs — Politique et configuration du cache ExoFS (no_std).
//!
//! Définit `CacheConfig`, `CachePolicy` et les seuils dynamiques (watermarks).
//! Règles : ARITH-02 (arithmétique vérifiée), RECUR-01 (zéro récursion).


use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// EvictionAlgorithmKind
// ─────────────────────────────────────────────────────────────────────────────

/// Algorithme d'éviction.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionAlgorithmKind {
    /// Least Recently Used.
    Lru   = 0,
    /// Least Frequently Used.
    Lfu   = 1,
    /// Adaptive Replacement Cache.
    Arc   = 2,
    /// CLOCK (approximation LRU bas coût).
    Clock = 3,
}

impl EvictionAlgorithmKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Lru),
            1 => Some(Self::Lfu),
            2 => Some(Self::Arc),
            3 => Some(Self::Clock),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lru   => "LRU",
            Self::Lfu   => "LFU",
            Self::Arc   => "ARC",
            Self::Clock => "CLOCK",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WritePolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Politique d'écriture.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePolicy {
    /// Écriture synchrone vers le stockage à chaque modification.
    WriteThrough = 0,
    /// Écriture différée — accumule les dirty pages.
    WriteBack    = 1,
}

impl WritePolicy {
    pub fn is_synchronous(self) -> bool {
        matches!(self, Self::WriteThrough)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PrefetchStrategy
// ─────────────────────────────────────────────────────────────────────────────

/// Stratégie de prélecture.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefetchStrategy {
    /// Aucune prélecture.
    None   = 0,
    /// Prélecture séquentielle (ahead_count blobs).
    Linear = 1,
    /// Prélecture adaptative basée sur les patterns d'accès.
    Adaptive = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration complète du cache.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Taille maximale en octets.
    pub max_bytes:        u64,
    /// Algorithme d'éviction.
    pub eviction_algo:    EvictionAlgorithmKind,
    /// Politique d'écriture.
    pub write_policy:     WritePolicy,
    /// Stratégie de prélecture.
    pub prefetch_strategy: PrefetchStrategy,
    /// Nombre de blobs à pré-charger.
    pub prefetch_ahead:   u32,
    /// Pourcentage maximum de pages sales avant flush forcé (0–100).
    pub dirty_ratio:      u8,
    /// Pourcentage minimum libre avant éviction proactive (0–100).
    pub min_free_ratio:   u8,
    /// Si `true`, invalide les entrées périmées après `ttl_ticks`.
    pub ttl_enabled:      bool,
    /// Durée de vie en ticks CPU (0 = infini).
    pub ttl_ticks:        u64,
}

impl CacheConfig {
    /// Configuration standard 512 MiB avec ARC.
    pub fn default_512mib() -> Self {
        Self {
            max_bytes:         512 * 1024 * 1024,
            eviction_algo:     EvictionAlgorithmKind::Arc,
            write_policy:      WritePolicy::WriteBack,
            prefetch_strategy: PrefetchStrategy::Linear,
            prefetch_ahead:    4,
            dirty_ratio:       40,
            min_free_ratio:    10,
            ttl_enabled:       false,
            ttl_ticks:         0,
        }
    }

    /// Configuration minimale 32 MiB avec LRU — convient aux tests.
    pub fn minimal() -> Self {
        Self {
            max_bytes:         32 * 1024 * 1024,
            eviction_algo:     EvictionAlgorithmKind::Lru,
            write_policy:      WritePolicy::WriteThrough,
            prefetch_strategy: PrefetchStrategy::None,
            prefetch_ahead:    0,
            dirty_ratio:       20,
            min_free_ratio:    20,
            ttl_enabled:       false,
            ttl_ticks:         0,
        }
    }

    /// Configuration pour environnement mémoire contraint.
    pub fn low_memory() -> Self {
        Self {
            max_bytes:         4 * 1024 * 1024,
            eviction_algo:     EvictionAlgorithmKind::Clock,
            write_policy:      WritePolicy::WriteThrough,
            prefetch_strategy: PrefetchStrategy::None,
            prefetch_ahead:    0,
            dirty_ratio:       10,
            min_free_ratio:    30,
            ttl_enabled:       true,
            ttl_ticks:         500_000,
        }
    }

    /// Valide la configuration. Retourne `InvalidArgument` si incohérente.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_bytes == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let sum = (self.dirty_ratio as u32)
            .checked_add(self.min_free_ratio as u32)
            .ok_or(ExofsError::OffsetOverflow)?;
        if sum > 100 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CachePolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Politique d'éviction avec seuils calculés.
#[derive(Clone, Debug)]
pub struct CachePolicy {
    pub config: CacheConfig,
}

impl CachePolicy {
    /// Construit une politique depuis une configuration.
    pub fn new(config: CacheConfig) -> Self {
        Self { config }
    }

    /// Seuil haut : éviction proactive si `used > high_watermark()`.
    pub fn high_watermark(&self) -> u64 {
        let pct = (100u64).saturating_sub(self.config.min_free_ratio as u64);
        self.config.max_bytes / 100 * pct
    }

    /// Seuil bas : arrêt de l'éviction quand `used < low_watermark()`.
    pub fn low_watermark(&self) -> u64 {
        self.config.max_bytes / 100 * 80
    }

    /// Limite de pages sales avant flush forcé.
    pub fn dirty_limit(&self) -> u64 {
        self.config.max_bytes / 100 * self.config.dirty_ratio as u64
    }

    /// `true` si la pression mémoire est élevée.
    pub fn is_under_pressure(&self, used: u64) -> bool {
        used >= self.high_watermark()
    }

    /// Quantité à libérer pour atteindre `low_watermark`.
    pub fn bytes_to_free(&self, used: u64) -> u64 {
        let low = self.low_watermark();
        if used <= low { return 0; }
        used.saturating_sub(low)
    }

    /// `true` si la TTL est activée et que l'entrée est expirée.
    pub fn is_expired(&self, inserted_at: u64, now: u64) -> bool {
        if !self.config.ttl_enabled || self.config.ttl_ticks == 0 {
            return false;
        }
        now.saturating_sub(inserted_at) >= self.config.ttl_ticks
    }

    /// `true` si l'écriture en writeback est en retard.
    pub fn needs_writeback(&self, dirty_bytes: u64) -> bool {
        self.config.write_policy == WritePolicy::WriteBack
            && dirty_bytes >= self.dirty_limit()
    }

    /// Accès à la configuration.
    pub fn config(&self) -> &CacheConfig { &self.config }

    /// Modifie la taille max à chaud.
    pub fn resize(&mut self, new_max_bytes: u64) -> ExofsResult<()> {
        if new_max_bytes == 0 { return Err(ExofsError::InvalidArgument); }
        self.config.max_bytes = new_max_bytes;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheTier — niveaux de cache
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau de cache (hiérarchie L1/L2/L3).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CacheTier {
    /// Cache chaud en RAM (plus rapide).
    Hot  = 0,
    /// Cache tiède (fréquent mais pas récent).
    Warm = 1,
    /// Cache froid (rare, candidat à l'éviction).
    Cold = 2,
}

impl CacheTier {
    /// Détermine le tier d'une entrée à partir de son accès récent et sa fréquence.
    pub fn classify(access_count: u64, ticks_since_last: u64, hot_threshold: u64) -> Self {
        if ticks_since_last <= hot_threshold && access_count >= 4 {
            Self::Hot
        } else if access_count >= 2 {
            Self::Warm
        } else {
            Self::Cold
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_high_watermark_below_max() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert!(p.high_watermark() < p.config.max_bytes);
    }

    #[test] fn test_low_watermark_below_high() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert!(p.low_watermark() <= p.high_watermark());
    }

    #[test] fn test_dirty_limit() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert!(p.dirty_limit() > 0);
        assert!(p.dirty_limit() < p.config.max_bytes);
    }

    #[test] fn test_is_under_pressure_false() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert!(!p.is_under_pressure(0));
    }

    #[test] fn test_is_under_pressure_true() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert!(p.is_under_pressure(p.config.max_bytes));
    }

    #[test] fn test_bytes_to_free_zero_when_not_pressured() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        assert_eq!(p.bytes_to_free(0), 0);
    }

    #[test] fn test_bytes_to_free_positive() {
        let p = CachePolicy::new(CacheConfig::default_512mib());
        let used = p.config.max_bytes;
        assert!(p.bytes_to_free(used) > 0);
    }

    #[test] fn test_ttl_not_expired_when_disabled() {
        let p = CachePolicy::new(CacheConfig::minimal());
        assert!(!p.is_expired(0, u64::MAX));
    }

    #[test] fn test_ttl_expired_when_enabled() {
        let mut cfg = CacheConfig::minimal();
        cfg.ttl_enabled = true;
        cfg.ttl_ticks   = 100;
        let p = CachePolicy::new(cfg);
        assert!(p.is_expired(0, 200));
        assert!(!p.is_expired(150, 200));
    }

    #[test] fn test_validate_ok() {
        assert!(CacheConfig::default_512mib().validate().is_ok());
    }

    #[test] fn test_validate_zero_bytes() {
        let mut cfg = CacheConfig::minimal();
        cfg.max_bytes = 0;
        assert!(cfg.validate().is_err());
    }

    #[test] fn test_resize() {
        let mut p = CachePolicy::new(CacheConfig::minimal());
        p.resize(64 * 1024 * 1024).unwrap();
        assert_eq!(p.config.max_bytes, 64 * 1024 * 1024);
    }

    #[test] fn test_cache_tier_classify() {
        assert_eq!(CacheTier::classify(5, 10, 100), CacheTier::Hot);
        assert_eq!(CacheTier::classify(2, 1000, 100), CacheTier::Warm);
        assert_eq!(CacheTier::classify(1, 9999, 100), CacheTier::Cold);
    }

    #[test] fn test_eviction_algorithm_name() {
        assert_eq!(EvictionAlgorithmKind::Lru.name(), "LRU");
        assert_eq!(EvictionAlgorithmKind::Arc.name(), "ARC");
    }

    #[test] fn test_write_policy_is_synchronous() {
        assert!(WritePolicy::WriteThrough.is_synchronous());
        assert!(!WritePolicy::WriteBack.is_synchronous());
    }
}

