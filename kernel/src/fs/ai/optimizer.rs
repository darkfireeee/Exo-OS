//! Real-Time Optimizer for Cache and Allocation Decisions
//!
//! Uses ML predictions to optimize:
//! - Cache eviction policies (which pages to evict)
//! - Prefetch aggressiveness (how much to prefetch)
//! - Read-ahead window sizes
//! - Cache partitioning (per-inode allocation)
//!
//! ## Decision Engine
//! Combines multiple signals:
//! 1. ML model predictions (future access likelihood)
//! 2. Historical access patterns (frequency, recency)
//! 3. System resource constraints (available memory)
//! 4. Workload characteristics (sequential vs random)
//!
//! ## Performance
//! - Decision latency: < 5µs
//! - No heap allocations in hot path
//! - Lock-free where possible

use super::predictor::{PrefetchPrediction, AccessPredictor};
use super::profiler::FeatureDescription;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use alloc::vec::Vec;
use crate::fs::utils::{log2_approx_f32, log2_approx_f64};

/// Cache decision for a page
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    /// Keep in cache with given priority
    Keep { priority: u8 },
    /// Evict from cache
    Evict,
    /// Promote to higher cache tier
    Promote,
    /// Demote to lower cache tier
    Demote,
}

/// Prefetch decision
#[derive(Debug, Clone, Copy)]
pub struct PrefetchDecision {
    /// Offsets to prefetch
    pub offsets: [u64; 4],
    /// Number of valid offsets
    pub count: usize,
    /// Total bytes to prefetch
    pub total_bytes: usize,
    /// Aggressiveness (0-100)
    pub aggressiveness: u8,
}

impl Default for PrefetchDecision {
    fn default() -> Self {
        Self {
            offsets: [0; 4],
            count: 0,
            total_bytes: 0,
            aggressiveness: 50,
        }
    }
}

/// Real-time cache and I/O optimizer
pub struct Optimizer {
    /// Statistics
    stats: OptimizerStats,
    /// Configuration
    config: OptimizerConfig,
}

/// Optimizer configuration
#[derive(Debug, Clone, Copy)]
pub struct OptimizerConfig {
    /// Minimum confidence for prefetch (0.0 - 1.0)
    pub min_prefetch_confidence: f32,
    /// Maximum prefetch size (bytes)
    pub max_prefetch_bytes: usize,
    /// Enable aggressive prefetching for sequential patterns
    pub aggressive_sequential: bool,
    /// Cache priority boost for ML-predicted hot pages
    pub ml_priority_boost: u8,
    /// Eviction threshold (lower = more aggressive)
    pub eviction_threshold: f32,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            min_prefetch_confidence: 0.6,
            max_prefetch_bytes: 1024 * 1024, // 1 MB
            aggressive_sequential: true,
            ml_priority_boost: 50,
            eviction_threshold: 0.3,
        }
    }
}

impl Optimizer {
    /// Create new optimizer with default config
    pub fn new() -> Self {
        Self::with_config(OptimizerConfig::default())
    }

    /// Create optimizer with custom config
    pub fn with_config(config: OptimizerConfig) -> Self {
        Self {
            stats: OptimizerStats::new(),
            config,
        }
    }

    /// Decide whether to keep or evict a page from cache
    ///
    /// # Arguments
    /// - `ino`: Inode number
    /// - `offset`: Page offset
    /// - `access_count`: Number of times page has been accessed
    /// - `last_access_ns`: Timestamp of last access
    /// - `ml_score`: ML prediction score for future access (0.0 - 1.0)
    ///
    /// # Returns
    /// Cache decision (Keep/Evict/Promote/Demote)
    ///
    /// # Performance
    /// Target: < 1µs (no allocations, simple arithmetic)
    pub fn decide_cache_action(
        &self,
        ino: u64,
        offset: u64,
        access_count: u32,
        last_access_ns: u64,
        ml_score: f32,
    ) -> CacheDecision {
        let start = crate::time::uptime_ns();

        // Calculate recency score (exponential decay)
        let now = crate::time::uptime_ns();
        let age_ns = now.saturating_sub(last_access_ns);
        let age_seconds = age_ns / 1_000_000_000;

        // Recency score: 1.0 (just accessed) -> 0.0 (very old)
        // Half-life of 60 seconds
        let recency_score = Self::exponential_decay(age_seconds as f32, 60.0);

        // Frequency score (logarithmic)
        let frequency_score = (log2_approx_f32(access_count as f32 + 1.0) / 10.0).min(1.0);

        // Combined score: weighted average
        let combined_score =
            ml_score * 0.4 +           // ML prediction (40%)
            recency_score * 0.35 +     // Recency (35%)
            frequency_score * 0.25;    // Frequency (25%)

        // Decision thresholds
        let decision = if combined_score > 0.8 {
            // Very hot page - promote to higher tier
            self.stats.promotions.fetch_add(1, Ordering::Relaxed);
            CacheDecision::Promote
        } else if combined_score > 0.5 {
            // Hot page - keep with high priority
            let priority = (combined_score * 255.0) as u8;
            self.stats.keeps.fetch_add(1, Ordering::Relaxed);
            CacheDecision::Keep { priority }
        } else if combined_score > self.config.eviction_threshold {
            // Warm page - keep with lower priority
            let priority = (combined_score * 128.0) as u8;
            self.stats.keeps.fetch_add(1, Ordering::Relaxed);
            CacheDecision::Keep { priority }
        } else if combined_score > 0.1 {
            // Cool page - demote to lower tier
            self.stats.demotions.fetch_add(1, Ordering::Relaxed);
            CacheDecision::Demote
        } else {
            // Cold page - evict
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            CacheDecision::Evict
        };

        let elapsed = crate::time::uptime_ns() - start;
        self.stats.total_decisions.fetch_add(1, Ordering::Relaxed);
        self.stats.total_decision_time_ns.fetch_add(elapsed, Ordering::Relaxed);

        decision
    }

    /// Decide prefetch strategy based on predictions
    ///
    /// # Arguments
    /// - `predictions`: ML model predictions
    /// - `pattern`: Detected access pattern
    /// - `available_memory_bytes`: Available cache memory
    ///
    /// # Returns
    /// Prefetch decision with offsets and aggressiveness
    pub fn decide_prefetch(
        &self,
        predictions: &[PrefetchPrediction],
        pattern: &FeatureDescription,
        available_memory_bytes: usize,
    ) -> PrefetchDecision {
        let start = crate::time::uptime_ns();

        let mut decision = PrefetchDecision::default();

        // Filter predictions by confidence
        let high_confidence: Vec<_> = predictions
            .iter()
            .filter(|p| p.confidence >= self.config.min_prefetch_confidence)
            .take(4)
            .collect();

        if high_confidence.is_empty() {
            return decision;
        }

        // Calculate aggressiveness based on pattern
        let mut aggressiveness = 50u8;

        if pattern.is_sequential() {
            // Sequential patterns benefit from aggressive prefetch
            aggressiveness = if self.config.aggressive_sequential { 80 } else { 60 };
        } else if pattern.is_random() {
            // Random patterns need conservative prefetch
            aggressiveness = 30;
        } else if pattern.is_strided() {
            // Strided patterns benefit from moderate prefetch
            aggressiveness = 50;
        }

        // Adjust for available memory
        if available_memory_bytes < 64 * 1024 * 1024 {
            // Less than 64MB available - be conservative
            aggressiveness = aggressiveness / 2;
        }

        // Build prefetch list
        let mut total_bytes = 0usize;
        for (i, pred) in high_confidence.iter().enumerate() {
            if i >= 4 {
                break;
            }

            // Check if we've exceeded max prefetch size
            if total_bytes + pred.length > self.config.max_prefetch_bytes {
                break;
            }

            decision.offsets[i] = pred.offset;
            decision.count += 1;
            total_bytes += pred.length;
        }

        decision.total_bytes = total_bytes;
        decision.aggressiveness = aggressiveness;

        let elapsed = crate::time::uptime_ns() - start;
        self.stats.total_prefetch_decisions.fetch_add(1, Ordering::Relaxed);
        self.stats.total_prefetch_decision_time_ns.fetch_add(elapsed, Ordering::Relaxed);

        if decision.count > 0 {
            self.stats.prefetch_decisions_with_results.fetch_add(1, Ordering::Relaxed);
        }

        decision
    }

    /// Calculate optimal read-ahead window size
    ///
    /// # Arguments
    /// - `pattern`: Access pattern characteristics
    /// - `current_window`: Current read-ahead window size (bytes)
    ///
    /// # Returns
    /// Recommended read-ahead window size (bytes)
    pub fn optimal_readahead_window(
        &self,
        pattern: &FeatureDescription,
        current_window: usize,
    ) -> usize {
        // Base window sizes for different patterns
        let base_window = if pattern.is_sequential() {
            // Sequential: large window (128KB - 1MB)
            512 * 1024
        } else if pattern.is_strided() {
            // Strided: medium window (64KB - 256KB)
            128 * 1024
        } else {
            // Random: small window (4KB - 32KB)
            16 * 1024
        };

        // Adjust based on access density
        let density_factor = (1.0 + pattern.access_density.max(0.0)).min(2.0);
        let adjusted = (base_window as f32 * density_factor) as usize;

        // Smooth transitions (exponential moving average)
        let alpha = 0.3;
        let new_window = (alpha * adjusted as f32 + (1.0 - alpha) * current_window as f32) as usize;

        // Clamp to reasonable range (4KB - 2MB)
        new_window.max(4096).min(2 * 1024 * 1024)
    }

    /// Calculate cache partition size for an inode
    ///
    /// Based on predicted working set and access frequency
    ///
    /// # Returns
    /// Recommended cache reservation (bytes)
    pub fn cache_partition_size(
        &self,
        pattern: &FeatureDescription,
        total_cache_bytes: usize,
        num_active_inodes: usize,
    ) -> usize {
        if num_active_inodes == 0 {
            return 0;
        }

        // Base allocation: equal share
        let base_allocation = total_cache_bytes / num_active_inodes.max(1);

        // Working set size factor (from features)
        let working_set_factor = (1.0 + pattern.working_set.max(0.0)).min(3.0);

        // Access frequency factor
        let frequency_factor = (1.0 + pattern.access_frequency.max(0.0)).min(2.0);

        // Calculate adjusted allocation
        let adjusted = (base_allocation as f32 * working_set_factor * frequency_factor) as usize;

        // Cap at 25% of total cache for any single inode
        adjusted.min(total_cache_bytes / 4)
    }

    /// Exponential decay function
    ///
    /// y = e^(-x / half_life)
    #[inline(always)]
    fn exponential_decay(x: f32, half_life: f32) -> f32 {
        // Approximation: e^(-x/h) ≈ 1 / (1 + x/h) for small x
        // More accurate: use lookup table for common values
        let ratio = x / half_life;
        if ratio > 10.0 {
            0.0
        } else {
            1.0 / (1.0 + ratio)
        }
    }

    /// Get optimizer statistics
    pub fn stats(&self) -> OptimizerStats {
        self.stats.clone()
    }

    /// Get configuration
    pub fn config(&self) -> OptimizerConfig {
        self.config
    }

    /// Update configuration
    pub fn set_config(&mut self, config: OptimizerConfig) {
        self.config = config;
    }
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Optimizer statistics
#[derive(Debug)]
pub struct OptimizerStats {
    /// Total cache decisions made
    pub total_decisions: AtomicU64,
    /// Time spent making decisions (ns)
    pub total_decision_time_ns: AtomicU64,
    /// Decisions to keep pages
    pub keeps: AtomicU64,
    /// Decisions to evict pages
    pub evictions: AtomicU64,
    /// Decisions to promote pages
    pub promotions: AtomicU64,
    /// Decisions to demote pages
    pub demotions: AtomicU64,
    /// Prefetch decisions made
    pub total_prefetch_decisions: AtomicU64,
    /// Prefetch decisions with results
    pub prefetch_decisions_with_results: AtomicU64,
    /// Time spent on prefetch decisions (ns)
    pub total_prefetch_decision_time_ns: AtomicU64,
}

impl OptimizerStats {
    fn new() -> Self {
        Self {
            total_decisions: AtomicU64::new(0),
            total_decision_time_ns: AtomicU64::new(0),
            keeps: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            promotions: AtomicU64::new(0),
            demotions: AtomicU64::new(0),
            total_prefetch_decisions: AtomicU64::new(0),
            prefetch_decisions_with_results: AtomicU64::new(0),
            total_prefetch_decision_time_ns: AtomicU64::new(0),
        }
    }

    /// Average decision time (nanoseconds)
    pub fn avg_decision_time_ns(&self) -> u64 {
        let total = self.total_decisions.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_decision_time_ns.load(Ordering::Relaxed) / total
    }

    /// Eviction ratio
    pub fn eviction_ratio(&self) -> f32 {
        let total = self.total_decisions.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        self.evictions.load(Ordering::Relaxed) as f32 / total as f32
    }

    /// Prefetch effectiveness
    pub fn prefetch_effectiveness(&self) -> f32 {
        let total = self.total_prefetch_decisions.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let with_results = self.prefetch_decisions_with_results.load(Ordering::Relaxed);
        with_results as f32 / total as f32
    }
}

impl Clone for OptimizerStats {
    fn clone(&self) -> Self {
        Self {
            total_decisions: AtomicU64::new(self.total_decisions.load(Ordering::Relaxed)),
            total_decision_time_ns: AtomicU64::new(self.total_decision_time_ns.load(Ordering::Relaxed)),
            keeps: AtomicU64::new(self.keeps.load(Ordering::Relaxed)),
            evictions: AtomicU64::new(self.evictions.load(Ordering::Relaxed)),
            promotions: AtomicU64::new(self.promotions.load(Ordering::Relaxed)),
            demotions: AtomicU64::new(self.demotions.load(Ordering::Relaxed)),
            total_prefetch_decisions: AtomicU64::new(self.total_prefetch_decisions.load(Ordering::Relaxed)),
            prefetch_decisions_with_results: AtomicU64::new(self.prefetch_decisions_with_results.load(Ordering::Relaxed)),
            total_prefetch_decision_time_ns: AtomicU64::new(self.total_prefetch_decision_time_ns.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = Optimizer::new();
        assert!(optimizer.config.min_prefetch_confidence > 0.0);
    }

    #[test]
    fn test_cache_decision_hot_page() {
        let optimizer = Optimizer::new();

        // Hot page: high ML score, recent access, high frequency
        let decision = optimizer.decide_cache_action(
            1,      // ino
            0,      // offset
            100,    // access_count
            crate::time::uptime_ns(), // just accessed
            0.9,    // high ML score
        );

        match decision {
            CacheDecision::Keep { priority } => {
                assert!(priority > 128, "Hot page should have high priority");
            }
            CacheDecision::Promote => {
                // Also acceptable for very hot page
            }
            decision => {
                // Unexpected decision for hot page
                panic!("Test failure: Expected Keep or Promote for hot page, got {:?}", decision);
            }
        }
    }

    #[test]
    fn test_cache_decision_cold_page() {
        let optimizer = Optimizer::new();

        // Cold page: low ML score, old access, low frequency
        let old_timestamp = crate::time::uptime_ns().saturating_sub(300_000_000_000); // 5 min ago

        let decision = optimizer.decide_cache_action(
            1,      // ino
            0,      // offset
            1,      // access_count (only once)
            old_timestamp,
            0.1,    // low ML score
        );

        // Should evict or demote
        assert!(
            matches!(decision, CacheDecision::Evict | CacheDecision::Demote),
            "Cold page should be evicted or demoted"
        );
    }

    #[test]
    fn test_exponential_decay() {
        let score_0 = Optimizer::exponential_decay(0.0, 60.0);
        let score_60 = Optimizer::exponential_decay(60.0, 60.0);
        let score_120 = Optimizer::exponential_decay(120.0, 60.0);

        assert!(score_0 > score_60);
        assert!(score_60 > score_120);
        assert!(score_0 > 0.9); // Recent should be near 1.0
    }

    #[test]
    fn test_decision_performance() {
        let optimizer = Optimizer::new();

        let start = crate::time::uptime_ns();
        for _ in 0..1000 {
            let _ = optimizer.decide_cache_action(1, 0, 10, start, 0.5);
        }
        let elapsed = crate::time::uptime_ns() - start;

        let avg = elapsed / 1000;
        assert!(avg < 5000, "Decision too slow: {}ns", avg); // Should be < 5µs
    }
}
