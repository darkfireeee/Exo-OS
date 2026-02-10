//! Data Tiering System - Hot/Warm/Cold Data Classification
//!
//! Automatically classifies data into temperature tiers based on access frequency:
//! - HOT: Frequently accessed (keep in fast cache)
//! - WARM: Moderately accessed (can be evicted if needed)
//! - COLD: Rarely accessed (candidate for eviction or compression)
//!
//! ## Strategy
//! - Uses exponential decay for access frequency
//! - Promotes/demotes based on access patterns
//! - Integrates with cache eviction policy

use alloc::vec::Vec;
use hashbrown::HashMap;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use spin::RwLock;
use crate::fs::utils::math::exp_approx;

// ═══════════════════════════════════════════════════════════════════════════
// DATA TEMPERATURE TIERS
// ═══════════════════════════════════════════════════════════════════════════

/// Temperature tier for data
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DataTier {
    /// Cold data - Rarely accessed, candidate for eviction
    Cold = 0,
    /// Warm data - Moderately accessed
    Warm = 1,
    /// Hot data - Frequently accessed, keep in cache
    Hot = 2,
}

impl DataTier {
    /// Get tier from access count (using thresholds)
    pub fn from_access_count(count: u64) -> Self {
        if count >= 100 {
            DataTier::Hot
        } else if count >= 10 {
            DataTier::Warm
        } else {
            DataTier::Cold
        }
    }

    /// Get eviction priority (higher = more likely to evict)
    pub fn eviction_priority(&self) -> u32 {
        match self {
            DataTier::Cold => 100,  // Evict first
            DataTier::Warm => 50,   // Evict second
            DataTier::Hot => 1,     // Evict last
        }
    }

    /// Should this tier be compressed?
    pub fn should_compress(&self) -> bool {
        matches!(self, DataTier::Cold)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ACCESS TRACKING
// ═══════════════════════════════════════════════════════════════════════════

/// Access frequency tracker for a data block
struct AccessFrequency {
    /// Raw access count
    access_count: u64,
    /// Decayed access score (for time-weighted frequency)
    score: f32,
    /// Last access timestamp (ns)
    last_access: u64,
    /// Current tier
    tier: DataTier,
}

impl AccessFrequency {
    fn new() -> Self {
        Self {
            access_count: 0,
            score: 0.0,
            last_access: 0,
            tier: DataTier::Cold,
        }
    }

    /// Record an access and update score
    fn on_access(&mut self, now: u64) {
        self.access_count += 1;

        // Exponential decay based on time since last access
        let time_delta = if now > self.last_access {
            now - self.last_access
        } else {
            0
        };

        // Decay factor: half-life of 1 second (1e9 ns)
        let decay_factor = if time_delta > 0 {
            exp_approx(-((time_delta as f32) / 1e9))
        } else {
            1.0
        };

        // Update score with decayed value + new access
        self.score = self.score * decay_factor + 1.0;

        // Update tier based on score
        self.tier = if self.score >= 50.0 {
            DataTier::Hot
        } else if self.score >= 5.0 {
            DataTier::Warm
        } else {
            DataTier::Cold
        };

        self.last_access = now;
    }

    /// Decay the score periodically (called by background task)
    fn decay(&mut self, now: u64) {
        let time_delta = if now > self.last_access {
            now - self.last_access
        } else {
            0
        };

        let decay_factor = if time_delta > 0 {
            exp_approx(-((time_delta as f32) / 1e9))
        } else {
            1.0
        };

        self.score *= decay_factor;

        // Demote tier if score dropped
        self.tier = if self.score >= 50.0 {
            DataTier::Hot
        } else if self.score >= 5.0 {
            DataTier::Warm
        } else {
            DataTier::Cold
        };
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TIERING MANAGER
// ═══════════════════════════════════════════════════════════════════════════

/// Key for tracking a data block (inode + offset)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub ino: u64,
    pub offset: u64,
}

impl BlockKey {
    pub fn new(ino: u64, offset: u64) -> Self {
        Self { ino, offset }
    }
}

/// Tiering Manager - Tracks and classifies data by temperature
pub struct TieringManager {
    /// Access frequency tracking per block
    frequencies: RwLock<HashMap<BlockKey, AccessFrequency>>,
    /// Statistics
    stats: TieringStats,
    /// Last decay timestamp
    last_decay: AtomicU64,
}

impl TieringManager {
    /// Create new tiering manager
    pub fn new() -> Self {
        Self {
            frequencies: RwLock::new(HashMap::new()),
            stats: TieringStats::new(),
            last_decay: AtomicU64::new(0),
        }
    }

    /// Record an access to a block
    pub fn on_access(&self, ino: u64, offset: u64) {
        let key = BlockKey::new(ino, offset);
        let now = crate::time::uptime_ns();

        let mut frequencies = self.frequencies.write();
        let freq = frequencies.entry(key).or_insert_with(AccessFrequency::new);

        let old_tier = freq.tier;
        freq.on_access(now);
        let new_tier = freq.tier;

        // Update stats if tier changed
        if old_tier != new_tier {
            self.stats.on_tier_change(old_tier, new_tier);
        }

        // Update total accesses
        self.stats.total_accesses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get tier for a block
    pub fn get_tier(&self, ino: u64, offset: u64) -> DataTier {
        let key = BlockKey::new(ino, offset);
        let frequencies = self.frequencies.read();

        frequencies.get(&key)
            .map(|f| f.tier)
            .unwrap_or(DataTier::Cold)
    }

    /// Get all blocks in a specific tier
    pub fn get_blocks_in_tier(&self, tier: DataTier) -> Vec<BlockKey> {
        let frequencies = self.frequencies.read();

        frequencies.iter()
            .filter(|(_, freq)| freq.tier == tier)
            .map(|(key, _)| *key)
            .collect()
    }

    /// Run periodic decay on all blocks
    ///
    /// Should be called every second by a background task
    pub fn periodic_decay(&self) {
        let now = crate::time::uptime_ns();
        let last_decay = self.last_decay.load(Ordering::Relaxed);

        // Only decay once per second
        if now - last_decay < 1_000_000_000 {
            return;
        }

        let mut frequencies = self.frequencies.write();

        for (_, freq) in frequencies.iter_mut() {
            freq.decay(now);
        }

        self.last_decay.store(now, Ordering::Relaxed);
    }

    /// Remove tracking for a block (when evicted or deleted)
    pub fn remove_block(&self, ino: u64, offset: u64) {
        let key = BlockKey::new(ino, offset);
        let mut frequencies = self.frequencies.write();
        frequencies.remove(&key);
    }

    /// Get statistics
    pub fn stats(&self) -> TieringStats {
        self.stats.clone()
    }

    /// Get number of blocks per tier
    pub fn count_by_tier(&self) -> (usize, usize, usize) {
        let frequencies = self.frequencies.read();

        let mut hot = 0;
        let mut warm = 0;
        let mut cold = 0;

        for (_, freq) in frequencies.iter() {
            match freq.tier {
                DataTier::Hot => hot += 1,
                DataTier::Warm => warm += 1,
                DataTier::Cold => cold += 1,
            }
        }

        (hot, warm, cold)
    }
}

impl Default for TieringManager {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// STATISTICS
// ═══════════════════════════════════════════════════════════════════════════

/// Tiering statistics
pub struct TieringStats {
    /// Total accesses tracked
    pub total_accesses: AtomicU64,
    /// Blocks promoted to hot
    pub promoted_to_hot: AtomicU32,
    /// Blocks demoted to cold
    pub demoted_to_cold: AtomicU32,
    /// Blocks in hot tier
    pub hot_blocks: AtomicU32,
    /// Blocks in warm tier
    pub warm_blocks: AtomicU32,
    /// Blocks in cold tier
    pub cold_blocks: AtomicU32,
}

impl Clone for TieringStats {
    fn clone(&self) -> Self {
        Self {
            total_accesses: AtomicU64::new(self.total_accesses.load(Ordering::Relaxed)),
            promoted_to_hot: AtomicU32::new(self.promoted_to_hot.load(Ordering::Relaxed)),
            demoted_to_cold: AtomicU32::new(self.demoted_to_cold.load(Ordering::Relaxed)),
            hot_blocks: AtomicU32::new(self.hot_blocks.load(Ordering::Relaxed)),
            warm_blocks: AtomicU32::new(self.warm_blocks.load(Ordering::Relaxed)),
            cold_blocks: AtomicU32::new(self.cold_blocks.load(Ordering::Relaxed)),
        }
    }
}

impl TieringStats {
    fn new() -> Self {
        Self {
            total_accesses: AtomicU64::new(0),
            promoted_to_hot: AtomicU32::new(0),
            demoted_to_cold: AtomicU32::new(0),
            hot_blocks: AtomicU32::new(0),
            warm_blocks: AtomicU32::new(0),
            cold_blocks: AtomicU32::new(0),
        }
    }

    fn on_tier_change(&self, old_tier: DataTier, new_tier: DataTier) {
        // Update counters
        match old_tier {
            DataTier::Hot => self.hot_blocks.fetch_sub(1, Ordering::Relaxed),
            DataTier::Warm => self.warm_blocks.fetch_sub(1, Ordering::Relaxed),
            DataTier::Cold => self.cold_blocks.fetch_sub(1, Ordering::Relaxed),
        };

        match new_tier {
            DataTier::Hot => {
                self.hot_blocks.fetch_add(1, Ordering::Relaxed);
                if old_tier != DataTier::Hot {
                    self.promoted_to_hot.fetch_add(1, Ordering::Relaxed);
                }
            }
            DataTier::Warm => {
                self.warm_blocks.fetch_add(1, Ordering::Relaxed);
            }
            DataTier::Cold => {
                self.cold_blocks.fetch_add(1, Ordering::Relaxed);
                if old_tier != DataTier::Cold {
                    self.demoted_to_cold.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// GLOBAL INSTANCE
// ═══════════════════════════════════════════════════════════════════════════

use spin::Once;

/// Global tiering manager
static TIERING_MANAGER: Once<TieringManager> = Once::new();

/// Initialize global tiering manager
pub fn init() {
    TIERING_MANAGER.call_once(|| TieringManager::new());
    log::debug!("Tiering manager initialized");
}

/// Get global tiering manager
pub fn get() -> &'static TieringManager {
    TIERING_MANAGER.get().expect("Tiering manager not initialized")
}

/// Record an access to a block
pub fn on_access(ino: u64, offset: u64) {
    get().on_access(ino, offset);
}

/// Get tier for a block
pub fn get_tier(ino: u64, offset: u64) -> DataTier {
    get().get_tier(ino, offset)
}

/// Get blocks in a specific tier
pub fn get_blocks_in_tier(tier: DataTier) -> Vec<BlockKey> {
    get().get_blocks_in_tier(tier)
}

/// Run periodic decay
pub fn periodic_decay() {
    get().periodic_decay();
}

/// Get statistics
pub fn stats() -> TieringStats {
    get().stats()
}
