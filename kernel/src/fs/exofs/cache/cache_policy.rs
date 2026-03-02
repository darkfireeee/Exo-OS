//! CachePolicy et CacheConfig — configuration du comportement du cache ExoFS (no_std).

/// Algorithme d'éviction.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionAlgorithmKind {
    Lru   = 0,   // Least Recently Used.
    Lfu   = 1,   // Least Frequently Used.
    Arc   = 2,   // Adaptive Replacement Cache.
    Clock = 3,   // CLOCK (approximation LRU bas coût).
}

/// Politique de write-back / write-through.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePolicy {
    WriteThrough = 0,
    WriteBack    = 1,
}

/// Configuration complète du cache.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub max_bytes:        u64,
    pub eviction_algo:    EvictionAlgorithmKind,
    pub write_policy:     WritePolicy,
    pub prefetch_ahead:   u32,   // Nombre de blobs à pré-charger.
    pub dirty_ratio:      u8,    // % max de pages sales avant flush forcé.
    pub min_free_ratio:   u8,    // % minimum libre avant éviction proactive.
}

impl CacheConfig {
    pub fn default_512mib() -> Self {
        Self {
            max_bytes:      512 * 1024 * 1024,
            eviction_algo:  EvictionAlgorithmKind::Arc,
            write_policy:   WritePolicy::WriteBack,
            prefetch_ahead: 4,
            dirty_ratio:    40,
            min_free_ratio: 10,
        }
    }

    pub fn minimal() -> Self {
        Self {
            max_bytes:      32 * 1024 * 1024,
            eviction_algo:  EvictionAlgorithmKind::Lru,
            write_policy:   WritePolicy::WriteThrough,
            prefetch_ahead: 0,
            dirty_ratio:    20,
            min_free_ratio: 20,
        }
    }
}

/// Politique d'éviction (wrapper avec seuils calculés).
#[derive(Clone, Debug)]
pub struct CachePolicy {
    pub config: CacheConfig,
}

impl CachePolicy {
    pub fn new(config: CacheConfig) -> Self {
        Self { config }
    }

    pub fn high_watermark(&self) -> u64 {
        self.config.max_bytes * (100 - self.config.min_free_ratio as u64) / 100
    }

    pub fn low_watermark(&self) -> u64 {
        self.config.max_bytes * 80 / 100
    }

    pub fn dirty_limit(&self) -> u64 {
        self.config.max_bytes * self.config.dirty_ratio as u64 / 100
    }
}
