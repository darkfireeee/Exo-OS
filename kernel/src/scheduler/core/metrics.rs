//! Scheduler Metrics - Lock-Free Performance Monitoring
//!
//! High-performance, lock-free metrics collection.
//! Better than Linux: Zero-cost metrics with atomic operations only.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Ordering used for relaxed counters (metrics don't need strict ordering)
const RELAXED: Ordering = Ordering::Relaxed;

/// Lock-free scheduler metrics
/// 
/// All fields are atomic for zero-lock access.
/// Inspired by kernel performance counters but fully lock-free.
pub struct SchedulerMetrics {
    // ═══════════════════════════════════════════════════════════════
    // Context Switch Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Total context switches performed
    pub context_switches: AtomicU64,
    /// Voluntary context switches (yield, sleep, etc.)
    pub voluntary_switches: AtomicU64,
    /// Involuntary context switches (preemption)
    pub involuntary_switches: AtomicU64,
    /// Context switch total latency (nanoseconds)
    pub switch_latency_total_ns: AtomicU64,
    /// Minimum switch latency observed
    pub switch_latency_min_ns: AtomicU64,
    /// Maximum switch latency observed
    pub switch_latency_max_ns: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Thread Lifecycle Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Total threads created
    pub threads_created: AtomicU64,
    /// Total threads terminated
    pub threads_terminated: AtomicU64,
    /// Current active thread count
    pub threads_active: AtomicUsize,
    /// Peak concurrent threads
    pub threads_peak: AtomicUsize,
    /// Zombie threads currently waiting
    pub zombies_current: AtomicUsize,
    /// Total zombies reaped
    pub zombies_reaped: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Queue Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Hot queue current size
    pub queue_hot_size: AtomicUsize,
    /// Normal queue current size
    pub queue_normal_size: AtomicUsize,
    /// Cold queue current size
    pub queue_cold_size: AtomicUsize,
    /// Pending queue current size (lock-free queue)
    pub queue_pending_size: AtomicUsize,
    /// Blocked threads count
    pub threads_blocked: AtomicUsize,
    
    // ═══════════════════════════════════════════════════════════════
    // Lock-Free Queue Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Successful CAS operations
    pub cas_successes: AtomicU64,
    /// CAS retries (contention indicator)
    pub cas_retries: AtomicU64,
    /// CAS failures (exhausted retries)
    pub cas_failures: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // CPU Time Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Total CPU time (nanoseconds)
    pub cpu_time_total_ns: AtomicU64,
    /// User-space CPU time
    pub cpu_time_user_ns: AtomicU64,
    /// Kernel-space CPU time
    pub cpu_time_kernel_ns: AtomicU64,
    /// Idle time (nanoseconds)
    pub cpu_time_idle_ns: AtomicU64,
    /// Time spent in scheduler code
    pub cpu_time_scheduler_ns: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Wait/Sleep Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Total wait operations (wait4, waitpid, etc.)
    pub wait_operations: AtomicU64,
    /// Total sleep operations
    pub sleep_operations: AtomicU64,
    /// Total time spent waiting (nanoseconds)
    pub wait_time_total_ns: AtomicU64,
    /// Total time spent sleeping (nanoseconds)
    pub sleep_time_total_ns: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Priority/Policy Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Priority changes
    pub priority_changes: AtomicU64,
    /// Policy changes
    pub policy_changes: AtomicU64,
    /// Priority inversions detected
    pub priority_inversions: AtomicU64,
    /// Priority inheritance activations
    pub priority_inheritance: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Migration/Affinity Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Thread migrations between CPUs
    pub thread_migrations: AtomicU64,
    /// Affinity changes
    pub affinity_changes: AtomicU64,
    /// Load balancing operations
    pub load_balance_ops: AtomicU64,
    
    // ═══════════════════════════════════════════════════════════════
    // Error Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Total errors encountered
    pub errors_total: AtomicU64,
    /// Recoverable errors
    pub errors_recoverable: AtomicU64,
    /// Critical errors
    pub errors_critical: AtomicU64,
}

impl SchedulerMetrics {
    /// Create new metrics with all counters at zero
    pub const fn new() -> Self {
        Self {
            // Context switches
            context_switches: AtomicU64::new(0),
            voluntary_switches: AtomicU64::new(0),
            involuntary_switches: AtomicU64::new(0),
            switch_latency_total_ns: AtomicU64::new(0),
            switch_latency_min_ns: AtomicU64::new(u64::MAX),
            switch_latency_max_ns: AtomicU64::new(0),
            
            // Thread lifecycle
            threads_created: AtomicU64::new(0),
            threads_terminated: AtomicU64::new(0),
            threads_active: AtomicUsize::new(0),
            threads_peak: AtomicUsize::new(0),
            zombies_current: AtomicUsize::new(0),
            zombies_reaped: AtomicU64::new(0),
            
            // Queues
            queue_hot_size: AtomicUsize::new(0),
            queue_normal_size: AtomicUsize::new(0),
            queue_cold_size: AtomicUsize::new(0),
            queue_pending_size: AtomicUsize::new(0),
            threads_blocked: AtomicUsize::new(0),
            
            // Lock-free operations
            cas_successes: AtomicU64::new(0),
            cas_retries: AtomicU64::new(0),
            cas_failures: AtomicU64::new(0),
            
            // CPU time
            cpu_time_total_ns: AtomicU64::new(0),
            cpu_time_user_ns: AtomicU64::new(0),
            cpu_time_kernel_ns: AtomicU64::new(0),
            cpu_time_idle_ns: AtomicU64::new(0),
            cpu_time_scheduler_ns: AtomicU64::new(0),
            
            // Wait/Sleep
            wait_operations: AtomicU64::new(0),
            sleep_operations: AtomicU64::new(0),
            wait_time_total_ns: AtomicU64::new(0),
            sleep_time_total_ns: AtomicU64::new(0),
            
            // Priority
            priority_changes: AtomicU64::new(0),
            policy_changes: AtomicU64::new(0),
            priority_inversions: AtomicU64::new(0),
            priority_inheritance: AtomicU64::new(0),
            
            // Migration
            thread_migrations: AtomicU64::new(0),
            affinity_changes: AtomicU64::new(0),
            load_balance_ops: AtomicU64::new(0),
            
            // Errors
            errors_total: AtomicU64::new(0),
            errors_recoverable: AtomicU64::new(0),
            errors_critical: AtomicU64::new(0),
        }
    }
    
    // ═══════════════════════════════════════════════════════════════
    // Increment Helpers (for common operations)
    // ═══════════════════════════════════════════════════════════════
    
    /// Record a context switch
    #[inline(always)]
    pub fn record_context_switch(&self, voluntary: bool, latency_ns: u64) {
        self.context_switches.fetch_add(1, RELAXED);
        if voluntary {
            self.voluntary_switches.fetch_add(1, RELAXED);
        } else {
            self.involuntary_switches.fetch_add(1, RELAXED);
        }
        self.switch_latency_total_ns.fetch_add(latency_ns, RELAXED);
        
        // Update min/max (approximate, may miss some due to races - acceptable for metrics)
        let _ = self.switch_latency_min_ns.fetch_update(RELAXED, RELAXED, |cur| {
            if latency_ns < cur { Some(latency_ns) } else { None }
        });
        let _ = self.switch_latency_max_ns.fetch_update(RELAXED, RELAXED, |cur| {
            if latency_ns > cur { Some(latency_ns) } else { None }
        });
    }
    
    /// Record thread creation
    #[inline(always)]
    pub fn record_thread_created(&self) {
        self.threads_created.fetch_add(1, RELAXED);
        let active = self.threads_active.fetch_add(1, RELAXED) + 1;
        
        // Update peak (approximate)
        let _ = self.threads_peak.fetch_update(RELAXED, RELAXED, |cur| {
            if active > cur { Some(active) } else { None }
        });
    }
    
    /// Record thread termination
    #[inline(always)]
    pub fn record_thread_terminated(&self) {
        self.threads_terminated.fetch_add(1, RELAXED);
        self.threads_active.fetch_sub(1, RELAXED);
    }
    
    /// Record CAS operation result
    #[inline(always)]
    pub fn record_cas(&self, success: bool, retries: u64) {
        if success {
            self.cas_successes.fetch_add(1, RELAXED);
        } else {
            self.cas_failures.fetch_add(1, RELAXED);
        }
        if retries > 0 {
            self.cas_retries.fetch_add(retries, RELAXED);
        }
    }
    
    /// Record an error
    #[inline(always)]
    pub fn record_error(&self, critical: bool) {
        self.errors_total.fetch_add(1, RELAXED);
        if critical {
            self.errors_critical.fetch_add(1, RELAXED);
        } else {
            self.errors_recoverable.fetch_add(1, RELAXED);
        }
    }
    
    // ═══════════════════════════════════════════════════════════════
    // Computed Metrics
    // ═══════════════════════════════════════════════════════════════
    
    /// Get average context switch latency (nanoseconds)
    pub fn avg_switch_latency_ns(&self) -> u64 {
        let total = self.switch_latency_total_ns.load(RELAXED);
        let count = self.context_switches.load(RELAXED);
        if count > 0 { total / count } else { 0 }
    }
    
    /// Get CAS success rate (0.0 to 1.0)
    pub fn cas_success_rate(&self) -> f64 {
        let success = self.cas_successes.load(RELAXED);
        let total = success + self.cas_failures.load(RELAXED);
        if total > 0 { success as f64 / total as f64 } else { 1.0 }
    }
    
    /// Get total queue size
    pub fn total_queued(&self) -> usize {
        self.queue_hot_size.load(RELAXED)
            + self.queue_normal_size.load(RELAXED)
            + self.queue_cold_size.load(RELAXED)
            + self.queue_pending_size.load(RELAXED)
    }
    
    /// Get scheduler overhead percentage
    pub fn scheduler_overhead_percent(&self) -> f64 {
        let total = self.cpu_time_total_ns.load(RELAXED);
        let sched = self.cpu_time_scheduler_ns.load(RELAXED);
        if total > 0 { (sched as f64 / total as f64) * 100.0 } else { 0.0 }
    }
    
    // ═══════════════════════════════════════════════════════════════
    // Snapshot & Reset
    // ═══════════════════════════════════════════════════════════════
    
    /// Get a snapshot of all metrics (for reporting)
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            context_switches: self.context_switches.load(RELAXED),
            voluntary_switches: self.voluntary_switches.load(RELAXED),
            involuntary_switches: self.involuntary_switches.load(RELAXED),
            avg_switch_latency_ns: self.avg_switch_latency_ns(),
            threads_created: self.threads_created.load(RELAXED),
            threads_terminated: self.threads_terminated.load(RELAXED),
            threads_active: self.threads_active.load(RELAXED),
            threads_peak: self.threads_peak.load(RELAXED),
            zombies_current: self.zombies_current.load(RELAXED),
            total_queued: self.total_queued(),
            cas_success_rate: self.cas_success_rate(),
            cas_retries: self.cas_retries.load(RELAXED),
            errors_total: self.errors_total.load(RELAXED),
            scheduler_overhead_percent: self.scheduler_overhead_percent(),
        }
    }
    
    /// Reset all metrics to zero (for benchmarking)
    pub fn reset(&self) {
        self.context_switches.store(0, RELAXED);
        self.voluntary_switches.store(0, RELAXED);
        self.involuntary_switches.store(0, RELAXED);
        self.switch_latency_total_ns.store(0, RELAXED);
        self.switch_latency_min_ns.store(u64::MAX, RELAXED);
        self.switch_latency_max_ns.store(0, RELAXED);
        self.threads_created.store(0, RELAXED);
        self.threads_terminated.store(0, RELAXED);
        // Don't reset threads_active - it's a current state, not a counter
        self.threads_peak.store(self.threads_active.load(RELAXED), RELAXED);
        self.zombies_reaped.store(0, RELAXED);
        self.cas_successes.store(0, RELAXED);
        self.cas_retries.store(0, RELAXED);
        self.cas_failures.store(0, RELAXED);
        self.cpu_time_total_ns.store(0, RELAXED);
        self.cpu_time_user_ns.store(0, RELAXED);
        self.cpu_time_kernel_ns.store(0, RELAXED);
        self.cpu_time_idle_ns.store(0, RELAXED);
        self.cpu_time_scheduler_ns.store(0, RELAXED);
        self.wait_operations.store(0, RELAXED);
        self.sleep_operations.store(0, RELAXED);
        self.wait_time_total_ns.store(0, RELAXED);
        self.sleep_time_total_ns.store(0, RELAXED);
        self.priority_changes.store(0, RELAXED);
        self.policy_changes.store(0, RELAXED);
        self.priority_inversions.store(0, RELAXED);
        self.priority_inheritance.store(0, RELAXED);
        self.thread_migrations.store(0, RELAXED);
        self.affinity_changes.store(0, RELAXED);
        self.load_balance_ops.store(0, RELAXED);
        self.errors_total.store(0, RELAXED);
        self.errors_recoverable.store(0, RELAXED);
        self.errors_critical.store(0, RELAXED);
    }
}

/// Metrics snapshot for reporting
#[derive(Debug, Clone, Copy)]
pub struct MetricsSnapshot {
    pub context_switches: u64,
    pub voluntary_switches: u64,
    pub involuntary_switches: u64,
    pub avg_switch_latency_ns: u64,
    pub threads_created: u64,
    pub threads_terminated: u64,
    pub threads_active: usize,
    pub threads_peak: usize,
    pub zombies_current: usize,
    pub total_queued: usize,
    pub cas_success_rate: f64,
    pub cas_retries: u64,
    pub errors_total: u64,
    pub scheduler_overhead_percent: f64,
}

impl MetricsSnapshot {
    /// Format as human-readable string
    pub fn format(&self) -> alloc::string::String {
        use alloc::format;
        format!(
            "Context Switches: {} (vol: {}, invol: {}, avg latency: {}ns)\n\
             Threads: {} active (peak: {}), {} created, {} terminated\n\
             Queued: {}, Zombies: {}\n\
             CAS: {:.2}% success, {} retries\n\
             Errors: {}, Scheduler overhead: {:.2}%",
            self.context_switches,
            self.voluntary_switches,
            self.involuntary_switches,
            self.avg_switch_latency_ns,
            self.threads_active,
            self.threads_peak,
            self.threads_created,
            self.threads_terminated,
            self.total_queued,
            self.zombies_current,
            self.cas_success_rate * 100.0,
            self.cas_retries,
            self.errors_total,
            self.scheduler_overhead_percent,
        )
    }
}

/// Global metrics instance (lock-free, always safe to access)
pub static METRICS: SchedulerMetrics = SchedulerMetrics::new();
