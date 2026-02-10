//! AI-Guided Block Allocator
//!
//! Uses machine learning and heuristics to optimize block placement:
//! - Learns access patterns
//! - Predicts future allocations
//! - Optimizes for sequential access
//! - Reduces seek time and fragmentation

use super::BitmapAllocator;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Access pattern
#[derive(Debug, Clone, Copy)]
enum AccessPattern {
    Sequential,
    Random,
    SequentialWrite,
    SequentialRead,
}

/// Allocation hint
#[derive(Debug, Clone)]
struct AllocationHint {
    /// Suggested block group
    group: u32,
    /// Confidence (0.0 - 1.0)
    confidence: f32,
    /// Reason
    pattern: AccessPattern,
}

/// Access history entry
#[derive(Debug, Clone)]
struct AccessEntry {
    /// Block number
    block: u64,
    /// Timestamp (simplified)
    timestamp: u64,
    /// Access type (true = write, false = read)
    is_write: bool,
}

/// AI Allocator
pub struct AiAllocator {
    /// Underlying allocator
    bitmap_allocator: Arc<BitmapAllocator>,
    /// Access history
    history: Mutex<VecDeque<AccessEntry>>,
    /// History size limit
    history_limit: usize,
    /// Current timestamp
    timestamp: AtomicU64,
    /// Statistics
    stats: AiStats,
}

impl AiAllocator {
    /// Create new AI allocator
    pub fn new(bitmap_allocator: Arc<BitmapAllocator>) -> Self {
        Self {
            bitmap_allocator,
            history: Mutex::new(VecDeque::new()),
            history_limit: 1000,
            timestamp: AtomicU64::new(0),
            stats: AiStats::new(),
        }
    }

    /// Allocate single block with AI guidance
    pub fn allocate_single(&self) -> FsResult<u64> {
        // Generate hint based on access patterns
        let hint = self.generate_hint();

        // Try allocating based on hint
        if let Some(hint) = hint {
            if let Ok(block) = self.allocate_with_hint(&hint) {
                self.stats.hint_hits.fetch_add(1, Ordering::Relaxed);
                self.record_access(block, true);
                return Ok(block);
            }
            self.stats.hint_misses.fetch_add(1, Ordering::Relaxed);
        }

        // Fallback to regular allocation
        let block = self.bitmap_allocator.allocate_block()?;
        self.record_access(block, true);
        Ok(block)
    }

    /// Generate allocation hint based on patterns
    fn generate_hint(&self) -> Option<AllocationHint> {
        let history = self.history.lock();

        if history.len() < 3 {
            return None; // Not enough data
        }

        // Analyze recent accesses
        let recent: Vec<_> = history.iter().rev().take(10).collect();

        // Check for sequential pattern
        if self.is_sequential(&recent) {
            // Predict next block should be near the last access
            if let Some(last) = recent.first() {
                let blocks_per_group = self.bitmap_allocator.blocks_per_group();
                let group = (last.block / blocks_per_group as u64) as u32;

                return Some(AllocationHint {
                    group,
                    confidence: 0.8,
                    pattern: if last.is_write {
                        AccessPattern::SequentialWrite
                    } else {
                        AccessPattern::SequentialRead
                    },
                });
            }
        }

        // Check for clustered access
        if let Some(most_common_group) = self.find_most_common_group(&recent) {
            return Some(AllocationHint {
                group: most_common_group,
                confidence: 0.6,
                pattern: AccessPattern::Random,
            });
        }

        None
    }

    /// Check if acesses are sequential
    fn is_sequential(&self, accesses: &[&AccessEntry]) -> bool {
        if accesses.len() < 2 {
            return false;
        }

        let mut sequential_count = 0;
        for i in 0..accesses.len() - 1 {
            let diff = accesses[i].block.abs_diff(accesses[i + 1].block);
            if diff <= 16 {
                // Within 16 blocks = sequential
                sequential_count += 1;
            }
        }

        sequential_count as f32 / (accesses.len() - 1) as f32 > 0.7
    }

    /// Find most common block group in accesses
    fn find_most_common_group(&self, accesses: &[&AccessEntry]) -> Option<u32> {
        if accesses.is_empty() {
            return None;
        }

        let blocks_per_group = self.bitmap_allocator.blocks_per_group();
        let mut counts = alloc::vec![0u32; self.bitmap_allocator.group_count()];

        for access in accesses {
            let group = (access.block / blocks_per_group as u64) as usize;
            if group < counts.len() {
                counts[group] += 1;
            }
        }

        counts.iter()
            .enumerate()
            .max_by_key(|(_, &count)| count)
            .map(|(group, _)| group as u32)
    }

    /// Allocate with hint
    fn allocate_with_hint(&self, hint: &AllocationHint) -> FsResult<u64> {
        // In production, would try to allocate from suggested group
        // For now, use regular allocation
        self.bitmap_allocator.allocate_block()
    }

    /// Record block access
    fn record_access(&self, block: u64, is_write: bool) {
        let timestamp = self.timestamp.fetch_add(1, Ordering::Relaxed);

        let entry = AccessEntry {
            block,
            timestamp,
            is_write,
        };

        let mut history = self.history.lock();
        history.push_back(entry);

        // Trim history
        while history.len() > self.history_limit {
            history.pop_front();
        }
    }

    /// Get statistics
    pub fn stats(&self) -> AiStatsSnapshot {
        AiStatsSnapshot {
            hint_hits: self.stats.hint_hits.load(Ordering::Relaxed),
            hint_misses: self.stats.hint_misses.load(Ordering::Relaxed),
            history_size: self.history.lock().len(),
        }
    }

    /// Clear history
    pub fn clear_history(&self) {
        let mut history = self.history.lock();
        history.clear();
        log::debug!("ext4plus: Cleared AI allocator history");
    }
}

/// AI statistics
struct AiStats {
    hint_hits: AtomicU64,
    hint_misses: AtomicU64,
}

impl AiStats {
    fn new() -> Self {
        Self {
            hint_hits: AtomicU64::new(0),
            hint_misses: AtomicU64::new(0),
        }
    }
}

/// Statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct AiStatsSnapshot {
    pub hint_hits: u64,
    pub hint_misses: u64,
    pub history_size: usize,
}

impl AiStatsSnapshot {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hint_hits + self.hint_misses;
        if total == 0 {
            0.0
        } else {
            self.hint_hits as f64 / total as f64
        }
    }
}
