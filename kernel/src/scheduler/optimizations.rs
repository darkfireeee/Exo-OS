//! Scheduler Optimizations Module
//!
//! Advanced performance optimizations for the V3 scheduler:
//! - NUMA-aware CPU selection
//! - Cache-optimized data structures
//! - Fast-path inlining
//! - Migration cost tracking
//! - Load balancing strategies

use crate::scheduler::thread::{Thread, ThreadId};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Cache line size for alignment (x86_64)
pub const CACHE_LINE_SIZE: usize = 64;

/// NUMA node distance threshold for remote placement
pub const NUMA_REMOTE_THRESHOLD: usize = 20;

/// Migration cost tracking window (microseconds)
pub const MIGRATION_COST_WINDOW_US: u64 = 1000;

/// Maximum migration cost before throttling (cycles)
pub const MAX_MIGRATION_COST_CYCLES: u64 = 5000;

/// Load balancing imbalance threshold (%)
pub const LOAD_IMBALANCE_THRESHOLD: usize = 20;

// ============================================================================
// NUMA-Aware CPU Selection
// ============================================================================

/// NUMA-aware CPU selector
/// 
/// Selects the best CPU for a thread considering:
/// - Thread's preferred NUMA node
/// - Current CPU load
/// - Memory locality
/// - Cache affinity
#[inline(always)]
pub fn select_cpu_numa_aware(
    thread: &Thread,
    available_cpus: &[usize],
) -> Option<usize> {
    // Fast path: Use thread's affinity if set
    if let Some(affinity_cpu) = thread.cpu_affinity() {
        if available_cpus.contains(&affinity_cpu) {
            return Some(affinity_cpu);
        }
    }

    // NUMA-aware selection
    #[cfg(feature = "numa")]
    {
        if let Some(preferred_node) = thread.numa_node() {
            // Try to find CPU on preferred node
            if let Some(cpu) = select_cpu_on_node(preferred_node, available_cpus) {
                return Some(cpu);
            }
        }
    }

    // Fallback: Select least loaded CPU
    select_least_loaded_cpu(available_cpus)
}

/// Select CPU on specific NUMA node
#[inline]
fn select_cpu_on_node(node_id: usize, available_cpus: &[usize]) -> Option<usize> {
    // Get CPUs for this NUMA node
    #[cfg(feature = "numa")]
    {
        use crate::scheduler::numa::NUMA_TOPOLOGY;
        
        if let Some(node) = NUMA_TOPOLOGY.node(node_id) {
            let node_cpus = node.cpus();
            
            // Find intersection of node CPUs and available CPUs
            for &cpu in node_cpus {
                if available_cpus.contains(&cpu) {
                    return Some(cpu);
                }
            }
        }
    }
    
    None
}

/// Select least loaded CPU from available list
#[inline]
fn select_least_loaded_cpu(available_cpus: &[usize]) -> Option<usize> {
    if available_cpus.is_empty() {
        return None;
    }

    // For now, simple round-robin
    // TODO: Use real CPU load metrics
    static NEXT_CPU: AtomicUsize = AtomicUsize::new(0);
    
    let idx = NEXT_CPU.fetch_add(1, Ordering::Relaxed) % available_cpus.len();
    Some(available_cpus[idx])
}

// ============================================================================
// Cache-Optimized Structures
// ============================================================================

/// Cache-aligned hot path data
/// 
/// This structure contains the most frequently accessed scheduler data,
/// aligned to cache line boundaries to prevent false sharing.
#[repr(C, align(64))]
pub struct HotPath {
    /// Current thread pointer (lock-free access)
    pub current_thread_id: AtomicU64,
    
    /// Context switch counter (for metrics)
    pub context_switches: AtomicU64,
    
    /// Last schedule timestamp (for timing)
    pub last_schedule_ns: AtomicU64,
    
    /// Padding to fill cache line (64 bytes total)
    _padding: [u8; 40],
}

impl HotPath {
    pub const fn new() -> Self {
        Self {
            current_thread_id: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            last_schedule_ns: AtomicU64::new(0),
            _padding: [0; 40],
        }
    }

    /// Fast path check if we need to schedule
    #[inline(always)]
    pub fn should_schedule(&self, current_ns: u64) -> bool {
        const SCHEDULE_QUANTUM_NS: u64 = 5_000_000; // 5ms
        
        let last_ns = self.last_schedule_ns.load(Ordering::Relaxed);
        current_ns.saturating_sub(last_ns) >= SCHEDULE_QUANTUM_NS
    }

    /// Update schedule timestamp
    #[inline(always)]
    pub fn mark_scheduled(&self, timestamp_ns: u64) {
        self.last_schedule_ns.store(timestamp_ns, Ordering::Relaxed);
        self.context_switches.fetch_add(1, Ordering::Relaxed);
    }
}

// ============================================================================
// Migration Cost Tracking
// ============================================================================

/// Migration cost tracker
/// 
/// Tracks the cost of thread migrations between CPUs to avoid
/// excessive cache thrashing.
pub struct MigrationCostTracker {
    /// Per-CPU migration counters
    migrations: [AtomicU64; 256],
    
    /// Per-CPU total migration cost (cycles)
    total_cost: [AtomicU64; 256],
    
    /// Window start timestamp
    window_start: AtomicU64,
}

impl MigrationCostTracker {
    pub const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            migrations: [ZERO; 256],
            total_cost: [ZERO; 256],
            window_start: AtomicU64::new(0),
        }
    }

    /// Record a migration cost
    #[inline]
    pub fn record_migration(&self, from_cpu: usize, to_cpu: usize, cost_cycles: u64) {
        if from_cpu < 256 && to_cpu < 256 {
            self.migrations[to_cpu].fetch_add(1, Ordering::Relaxed);
            self.total_cost[to_cpu].fetch_add(cost_cycles, Ordering::Relaxed);
        }
    }

    /// Get average migration cost for a CPU
    #[inline]
    pub fn average_cost(&self, cpu: usize) -> u64 {
        if cpu >= 256 {
            return 0;
        }

        let migrations = self.migrations[cpu].load(Ordering::Relaxed);
        if migrations == 0 {
            return 0;
        }

        let total = self.total_cost[cpu].load(Ordering::Relaxed);
        total / migrations
    }

    /// Check if migration should be throttled
    #[inline(always)]
    pub fn should_throttle(&self, cpu: usize) -> bool {
        self.average_cost(cpu) > MAX_MIGRATION_COST_CYCLES
    }

    /// Reset window (called periodically)
    pub fn reset_window(&self, current_ns: u64) {
        let last_reset = self.window_start.load(Ordering::Relaxed);
        if current_ns.saturating_sub(last_reset) > MIGRATION_COST_WINDOW_US * 1000 {
            // Reset all counters
            for i in 0..256 {
                self.migrations[i].store(0, Ordering::Relaxed);
                self.total_cost[i].store(0, Ordering::Relaxed);
            }
            self.window_start.store(current_ns, Ordering::Relaxed);
        }
    }
}

// ============================================================================
// Load Balancing
// ============================================================================

/// Load balancer state
pub struct LoadBalancer {
    /// Per-CPU thread counts
    cpu_loads: [AtomicUsize; 256],
    
    /// Last balance timestamp
    last_balance: AtomicU64,
}

impl LoadBalancer {
    pub const fn new() -> Self {
        const ZERO: AtomicUsize = AtomicUsize::new(0);
        Self {
            cpu_loads: [ZERO; 256],
            last_balance: AtomicU64::new(0),
        }
    }

    /// Update CPU load
    #[inline]
    pub fn set_load(&self, cpu: usize, load: usize) {
        if cpu < 256 {
            self.cpu_loads[cpu].store(load, Ordering::Relaxed);
        }
    }

    /// Get CPU load
    #[inline]
    pub fn get_load(&self, cpu: usize) -> usize {
        if cpu < 256 {
            self.cpu_loads[cpu].load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Check if load balancing is needed
    #[inline]
    pub fn needs_balancing(&self, num_cpus: usize) -> bool {
        if num_cpus < 2 {
            return false;
        }

        // Find min and max loads
        let mut min_load = usize::MAX;
        let mut max_load = 0;

        for cpu in 0..num_cpus {
            let load = self.get_load(cpu);
            min_load = min_load.min(load);
            max_load = max_load.max(load);
        }

        // Check imbalance threshold
        if max_load == 0 {
            return false;
        }

        let imbalance_pct = ((max_load - min_load) * 100) / max_load;
        imbalance_pct > LOAD_IMBALANCE_THRESHOLD
    }

    /// Find source and target CPUs for work stealing
    pub fn find_steal_pair(&self, num_cpus: usize) -> Option<(usize, usize)> {
        if num_cpus < 2 {
            return None;
        }

        let mut max_cpu = 0;
        let mut max_load = 0;
        let mut min_cpu = 0;
        let mut min_load = usize::MAX;

        for cpu in 0..num_cpus {
            let load = self.get_load(cpu);
            if load > max_load {
                max_load = load;
                max_cpu = cpu;
            }
            if load < min_load {
                min_load = load;
                min_cpu = cpu;
            }
        }

        if max_load > min_load + 1 {
            Some((max_cpu, min_cpu)) // (from, to)
        } else {
            None
        }
    }
}

// ============================================================================
// Fast Path Helpers
// ============================================================================

/// Check if current CPU is idle
#[inline(always)]
pub fn is_cpu_idle(cpu_id: usize) -> bool {
    // TODO: Read from per-CPU idle flag
    // For now, always return false
    false
}

/// Get current CPU ID
#[inline(always)]
pub fn current_cpu() -> usize {
    // Read from CPU-local storage
    #[cfg(target_arch = "x86_64")]
    {
        // Use APIC ID or GS-based per-CPU data
        // For now, stub to 0
        0
    }
    
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// Prefetch next thread context (cache warming)
#[inline(always)]
pub fn prefetch_thread_context(_thread: &Thread) {
    #[cfg(all(target_arch = "x86_64", target_feature = "sse"))]
    {
        // SSE prefetch intrinsic requires specific pointer type
        // Disabled for now - requires unstable features
        // let ctx_ptr = thread as *const Thread as *const i8;
        // unsafe {
        //     core::arch::x86_64::_mm_prefetch::<0>(ctx_ptr);
        // }
    }
}

// ============================================================================
// Branch Prediction Hints
// ============================================================================

/// Likely branch hint (for hot paths)
#[inline(always)]
pub const fn likely(b: bool) -> bool {
    // Use compiler built-in when stable
    // For now, just return the value
    b
}

/// Unlikely branch hint (for error paths)
#[inline(always)]
pub const fn unlikely(b: bool) -> bool {
    // Use compiler built-in when stable
    // For now, just return the value
    b
}

// ============================================================================
// Global Optimization State
// ============================================================================

/// Global optimization structures
pub struct GlobalOptimizations {
    /// Hot path data (per-CPU, for now single)
    pub hot_path: HotPath,
    
    /// Migration cost tracker
    pub migration_costs: MigrationCostTracker,
    
    /// Load balancer
    pub load_balancer: LoadBalancer,
}

impl GlobalOptimizations {
    pub const fn new() -> Self {
        Self {
            hot_path: HotPath::new(),
            migration_costs: MigrationCostTracker::new(),
            load_balancer: LoadBalancer::new(),
        }
    }
}

/// Global optimization instance
pub static GLOBAL_OPTIMIZATIONS: GlobalOptimizations = GlobalOptimizations::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_line_alignment() {
        use core::mem::{size_of, align_of};
        
        // HotPath should be cache-line aligned
        assert_eq!(align_of::<HotPath>(), 64);
        assert_eq!(size_of::<HotPath>(), 64);
    }

    #[test]
    fn test_migration_cost_tracker() {
        let tracker = MigrationCostTracker::new();
        
        // Record some migrations
        tracker.record_migration(0, 1, 1000);
        tracker.record_migration(0, 1, 2000);
        
        // Average should be 1500
        assert_eq!(tracker.average_cost(1), 1500);
        
        // Should not throttle yet
        assert!(!tracker.should_throttle(1));
    }

    #[test]
    fn test_load_balancer() {
        let balancer = LoadBalancer::new();
        
        // Set loads
        balancer.set_load(0, 10);
        balancer.set_load(1, 2);
        
        // Should need balancing (80% imbalance)
        assert!(balancer.needs_balancing(2));
        
        // Should suggest stealing from CPU 0 to CPU 1
        let pair = balancer.find_steal_pair(2);
        assert_eq!(pair, Some((0, 1)));
    }
}
