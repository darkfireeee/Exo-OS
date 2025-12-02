//! Adaptive Optimizer for POSIX-X
//!
//! Automatically adapts syscall strategies based on runtime patterns

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::RwLock;

/// Optimization strategy for syscalls
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationStrategy {
    /// Direct syscall - no optimization
    Direct,
    /// Batch multiple calls together
    Batched,
    /// Use zero-copy when possible
    ZeroCopy,
    /// Cache results
    Cached,
    /// Asynchronous execution
    Async,
}

/// Pattern detection for syscalls
#[derive(Debug, Clone)]
pub struct SyscallPattern {
    /// Syscall number
    pub syscall_num: usize,
    /// Call frequency (calls per second)
    pub frequency: f64,
    /// Average call duration (nanoseconds)
    pub avg_duration_ns: u64,
    /// Detected pattern
    pub pattern: PatternType,
    /// Recommended strategy
    pub recommended_strategy: OptimizationStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternType {
    /// Sequential reads/writes
    Sequential,
    /// Random access
    Random,
    /// Repeated calls with same args
    Repetitive,
    /// Burst of calls
    Bursty,
    /// Sparse/occasional calls
    Sparse,
}

/// Adaptive optimizer that learns from syscall patterns
pub struct AdaptiveOptimizer {
    /// Pattern database
    patterns: RwLock<BTreeMap<usize, SyscallPattern>>,
    /// Total calls per syscall
    call_counts: RwLock<BTreeMap<usize, AtomicU64>>,
    /// Total duration per syscall (ns)
    total_durations: RwLock<BTreeMap<usize, AtomicU64>>,
    /// Optimization enabled
    enabled: AtomicUsize,
}

impl AdaptiveOptimizer {
    /// Create new adaptive optimizer
    pub const fn new() -> Self {
        Self {
            patterns: RwLock::new(BTreeMap::new()),
            call_counts: RwLock::new(BTreeMap::new()),
            total_durations: RwLock::new(BTreeMap::new()),
            enabled: AtomicUsize::new(1),
        }
    }

    /// Record a syscall execution
    pub fn record_syscall(&self, syscall_num: usize, duration_ns: u64, args: &[u64]) {
        if self.enabled.load(Ordering::Relaxed) == 0 {
            return;
        }

        // Update call count
        {
            let mut counts = self.call_counts.write();
            counts
                .entry(syscall_num)
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        // Update total duration
        {
            let mut durations = self.total_durations.write();
            durations
                .entry(syscall_num)
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(duration_ns, Ordering::Relaxed);
        }

        // Analyze pattern every N calls
        let call_count = self
            .call_counts
            .read()
            .get(&syscall_num)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0);

        if call_count % 100 == 0 {
            self.analyze_pattern(syscall_num);
        }
    }

    /// Analyze syscall pattern and update recommendations
    fn analyze_pattern(&self, syscall_num: usize) {
        let call_count = self
            .call_counts
            .read()
            .get(&syscall_num)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0) as f64;

        let total_duration = self
            .total_durations
            .read()
            .get(&syscall_num)
            .map(|d| d.load(Ordering::Relaxed))
            .unwrap_or(0);

        if call_count == 0.0 {
            return;
        }

        let avg_duration = total_duration / call_count as u64;

        // Simple heuristics for pattern detection
        let frequency = call_count / 60.0; // Calls per minute (simplified)

        let (pattern, strategy) = if frequency > 100.0 {
            // High frequency -> batching
            (PatternType::Bursty, OptimizationStrategy::Batched)
        } else if avg_duration < 1000 {
            // Very fast calls -> caching
            (PatternType::Repetitive, OptimizationStrategy::Cached)
        } else if avg_duration > 1_000_000 {
            // Slow calls -> async
            (PatternType::Random, OptimizationStrategy::Async)
        } else {
            (PatternType::Sequential, OptimizationStrategy::Direct)
        };

        let pattern_data = SyscallPattern {
            syscall_num,
            frequency,
            avg_duration_ns: avg_duration,
            pattern,
            recommended_strategy: strategy,
        };

        self.patterns.write().insert(syscall_num, pattern_data);
    }

    /// Get optimization strategy for a syscall
    pub fn get_strategy(&self, syscall_num: usize) -> OptimizationStrategy {
        self.patterns
            .read()
            .get(&syscall_num)
            .map(|p| p.recommended_strategy)
            .unwrap_or(OptimizationStrategy::Direct)
    }

    /// Get pattern info for a syscall
    pub fn get_pattern(&self, syscall_num: usize) -> Option<SyscallPattern> {
        self.patterns.read().get(&syscall_num).cloned()
    }

    /// Enable/disable optimization
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled as usize, Ordering::Relaxed);
    }

    /// Get statistics for all syscalls
    pub fn get_statistics(&self) -> alloc::vec::Vec<SyscallPattern> {
        self.patterns.read().values().cloned().collect()
    }

    /// Clear all collected data
    pub fn reset(&self) {
        self.patterns.write().clear();
        self.call_counts.write().clear();
        self.total_durations.write().clear();
    }
}

/// Global adaptive optimizer instance
pub static ADAPTIVE_OPTIMIZER: AdaptiveOptimizer = AdaptiveOptimizer::new();

/// Record a syscall execution (convenience function)
#[inline]
pub fn record_syscall(syscall_num: usize, duration_ns: u64, args: &[u64]) {
    ADAPTIVE_OPTIMIZER.record_syscall(syscall_num, duration_ns, args);
}

/// Get optimization strategy for a syscall
#[inline]
pub fn get_strategy(syscall_num: usize) -> OptimizationStrategy {
    ADAPTIVE_OPTIMIZER.get_strategy(syscall_num)
}
