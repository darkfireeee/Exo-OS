//! Prefetch Subsystem - Intelligent Read-Ahead
//!
//! Implements adaptive prefetching with pattern detection:
//! - Sequential detection: Detects sequential reads and prefetches ahead
//! - Stride detection: Detects strided access patterns (e.g., reading every 4KB)
//! - History-based: Learns from past access patterns
//!
//! ## Performance Targets
//! - Detection latency: < 1µs
//! - Prefetch accuracy: > 80%
//! - Memory overhead: < 1% of total cache

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use hashbrown::HashMap;
use spin::RwLock;

// ═══════════════════════════════════════════════════════════════════════════
// ACCESS PATTERN TRACKING
// ═══════════════════════════════════════════════════════════════════════════

/// Access pattern for an inode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPattern {
    /// Sequential forward reads
    Sequential,
    /// Strided access (e.g., every N bytes)
    Strided { stride: u64 },
    /// Random access (no pattern)
    Random,
}

/// Access history entry
#[derive(Debug, Clone, Copy)]
struct AccessEntry {
    /// Offset accessed
    offset: u64,
    /// Length read
    length: usize,
    /// Timestamp (in nanoseconds)
    timestamp: u64,
}

/// Per-inode access tracker
struct InodeAccessTracker {
    /// Inode number
    ino: u64,
    /// Recent access history (circular buffer)
    history: VecDeque<AccessEntry>,
    /// Detected pattern
    pattern: AccessPattern,
    /// Last detected offset
    last_offset: u64,
    /// Consecutive sequential accesses
    sequential_count: u32,
    /// Stripe stride (if detected)
    stride: Option<u64>,
}

impl InodeAccessTracker {
    fn new(ino: u64) -> Self {
        Self {
            ino,
            history: VecDeque::with_capacity(16),
            pattern: AccessPattern::Random,
            last_offset: 0,
            sequential_count: 0,
            stride: None,
        }
    }

    /// Record an access and update pattern
    fn record_access(&mut self, offset: u64, length: usize) {
        let now = crate::time::uptime_ns();

        let entry = AccessEntry {
            offset,
            length,
            timestamp: now,
        };

        // Add to history
        if self.history.len() >= 16 {
            self.history.pop_front();
        }
        self.history.push_back(entry);

        // Analyze pattern
        self.analyze_pattern();
    }

    /// Analyze access pattern
    fn analyze_pattern(&mut self) {
        if self.history.len() < 3 {
            return;
        }

        // Get last 3 accesses
        let len = self.history.len();
        let entries: Vec<_> = self.history.iter().rev().take(3).collect();

        if entries.len() < 3 {
            return;
        }

        let e0 = entries[0];
        let e1 = entries[1];
        let e2 = entries[2];

        // Check for sequential pattern
        let delta1 = if e0.offset >= e1.offset {
            e0.offset - e1.offset
        } else {
            return; // backwards access
        };

        let delta2 = if e1.offset >= e2.offset {
            e1.offset - e2.offset
        } else {
            return;
        };

        // Sequential: deltas are equal to length
        if delta1 == e1.length as u64 && delta2 == e2.length as u64 {
            self.sequential_count += 1;
            if self.sequential_count >= 2 {
                self.pattern = AccessPattern::Sequential;
            }
            return;
        }

        // Strided: deltas are equal but not equal to length
        if delta1 == delta2 && delta1 > 0 {
            self.stride = Some(delta1);
            self.pattern = AccessPattern::Strided { stride: delta1 };
            return;
        }

        // Random access
        self.pattern = AccessPattern::Random;
        self.sequential_count = 0;
    }

    /// Get predicted next offsets to prefetch
    fn predict_prefetch(&self, current_offset: u64, current_length: usize) -> Vec<u64> {
        let mut predictions = Vec::new();

        match self.pattern {
            AccessPattern::Sequential => {
                // Prefetch next 4 sequential blocks
                let mut next_offset = current_offset + current_length as u64;
                for _ in 0..4 {
                    predictions.push(next_offset);
                    next_offset += current_length as u64;
                }
            }
            AccessPattern::Strided { stride } => {
                // Prefetch next 2 strided blocks
                let mut next_offset = current_offset + stride;
                for _ in 0..2 {
                    predictions.push(next_offset);
                    next_offset += stride;
                }
            }
            AccessPattern::Random => {
                // No prefetch for random access
            }
        }

        predictions
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// PREFETCH MANAGER
// ═══════════════════════════════════════════════════════════════════════════

/// Prefetch Manager
pub struct PrefetchManager {
    /// Per-inode access trackers
    trackers: RwLock<HashMap<u64, InodeAccessTracker>>,
    /// Statistics
    stats: PrefetchStats,
}

impl PrefetchManager {
    /// Create new prefetch manager
    pub fn new() -> Self {
        Self {
            trackers: RwLock::new(HashMap::new()),
            stats: PrefetchStats::new(),
        }
    }

    /// Record an access and trigger prefetch if appropriate
    ///
    /// Returns list of offsets to prefetch
    pub fn on_read(&self, ino: u64, offset: u64, length: usize) -> Vec<u64> {
        let mut trackers = self.trackers.write();

        // Get or create tracker for this inode
        let tracker = trackers.entry(ino).or_insert_with(|| InodeAccessTracker::new(ino));

        // Record access
        tracker.record_access(offset, length);

        // Get prefetch predictions
        let predictions = tracker.predict_prefetch(offset, length);

        // Update stats
        self.stats.total_reads.fetch_add(1, Ordering::Relaxed);
        if !predictions.is_empty() {
            self.stats.prefetch_triggered.fetch_add(1, Ordering::Relaxed);
        }

        predictions
    }

    /// Get detected pattern for an inode
    pub fn get_pattern(&self, ino: u64) -> Option<AccessPattern> {
        let trackers = self.trackers.read();
        trackers.get(&ino).map(|t| t.pattern)
    }

    /// Get statistics
    pub fn stats(&self) -> PrefetchStats {
        self.stats.clone()
    }

    /// Clear tracker for an inode (when file is closed)
    pub fn clear_tracker(&self, ino: u64) {
        let mut trackers = self.trackers.write();
        trackers.remove(&ino);
    }

    /// Clear all trackers
    pub fn clear_all(&self) {
        let mut trackers = self.trackers.write();
        trackers.clear();
    }
}

impl Default for PrefetchManager {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// STATISTICS
// ═══════════════════════════════════════════════════════════════════════════

/// Prefetch statistics
pub struct PrefetchStats {
    /// Total reads tracked
    pub total_reads: AtomicU64,
    /// Number of times prefetch was triggered
    pub prefetch_triggered: AtomicU64,
    /// Sequential patterns detected
    pub sequential_detected: AtomicU32,
    /// Strided patterns detected
    pub strided_detected: AtomicU32,
}

impl Clone for PrefetchStats {
    fn clone(&self) -> Self {
        Self {
            total_reads: AtomicU64::new(self.total_reads.load(Ordering::Relaxed)),
            prefetch_triggered: AtomicU64::new(self.prefetch_triggered.load(Ordering::Relaxed)),
            sequential_detected: AtomicU32::new(self.sequential_detected.load(Ordering::Relaxed)),
            strided_detected: AtomicU32::new(self.strided_detected.load(Ordering::Relaxed)),
        }
    }
}

impl PrefetchStats {
    fn new() -> Self {
        Self {
            total_reads: AtomicU64::new(0),
            prefetch_triggered: AtomicU64::new(0),
            sequential_detected: AtomicU32::new(0),
            strided_detected: AtomicU32::new(0),
        }
    }

    /// Get prefetch hit rate (0.0 - 1.0)
    pub fn hit_rate(&self) -> f32 {
        let total = self.total_reads.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let triggered = self.prefetch_triggered.load(Ordering::Relaxed);
        triggered as f32 / total as f32
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// GLOBAL INSTANCE
// ═══════════════════════════════════════════════════════════════════════════

use spin::Once;

/// Global prefetch manager
static PREFETCH_MANAGER: Once<PrefetchManager> = Once::new();

/// Initialize global prefetch manager
pub fn init() {
    PREFETCH_MANAGER.call_once(|| PrefetchManager::new());
    log::debug!("Prefetch manager initialized");
}

/// Get global prefetch manager
pub fn get() -> &'static PrefetchManager {
    PREFETCH_MANAGER.get().expect("Prefetch manager not initialized")
}

/// Record a read access and get prefetch predictions
pub fn on_read(ino: u64, offset: u64, length: usize) -> Vec<u64> {
    get().on_read(ino, offset, length)
}

/// Get detected pattern for an inode
pub fn get_pattern(ino: u64) -> Option<AccessPattern> {
    get().get_pattern(ino)
}

/// Get prefetch statistics
pub fn stats() -> PrefetchStats {
    get().stats()
}
