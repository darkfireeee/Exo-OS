//! Scheduler Core Module - Exo-OS Linux Crusher
//!
//! High-performance, lock-free scheduler with advanced features:
//!
//! ## Features
//! - **Lock-free pending queue** (AtomicPtr + CAS for fork safety)
//! - **3-queue EMA prediction** (Hot/Normal/Cold based on runtime)
//! - **Multiple scheduling policies** (FIFO, RR, Normal, Batch, Idle, Deadline)
//! - **Comprehensive error handling** with recovery hints
//! - **Lock-free metrics** for zero-overhead monitoring
//! - **Load balancing** for multi-CPU systems
//! - **Thread limits** and automatic zombie cleanup
//!
//! ## Architecture
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     Scheduler Core                          │
//! ├─────────────────────────────────────────────────────────────┤
//! │  scheduler.rs    Main scheduler (V3) - lock-free fork-safe │
//! │  error.rs        Typed error handling with recovery hints  │
//! │  metrics.rs      Lock-free performance metrics             │
//! │  policy.rs       Scheduling policies (FIFO, RR, CFS, EDF)  │
//! │  loadbalancer.rs Multi-CPU load balancing                  │
//! │  affinity.rs     CPU affinity management                   │
//! │  predictive.rs   EMA-based runtime prediction              │
//! │  statistics.rs   Legacy statistics (mutex-based)           │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why Better Than Linux
//! 1. **Simpler**: ~1000 lines vs Linux's 10,000+ lines scheduler
//! 2. **Faster**: Lock-free fork, 304-cycle context switch target
//! 3. **Safer**: Typed errors, no silent failures
//! 4. **Cleaner**: No legacy cruft, modern Rust design

pub mod scheduler;
pub mod error;
pub mod metrics;
pub mod policy;
pub mod loadbalancer;
pub mod affinity;
pub mod statistics;
pub mod predictive;

// Keep scheduler_v2 for reference but don't export it
mod scheduler_v2;

// ═══════════════════════════════════════════════════════════════
// Main Scheduler Exports (V3 - Production)
// ═══════════════════════════════════════════════════════════════

pub use scheduler::{
    // Main scheduler
    Scheduler, 
    SCHEDULER, 
    init, 
    start, 
    yield_now, 
    block_current, 
    unblock,
    
    // Queue types
    QueueType,
    
    // Stats (legacy - Copy/Clone)
    SchedulerStats,
    
    // Atomic stats (lock-free)
    AtomicSchedulerStats,
    
    // Limits
    MAX_THREADS,
    MAX_PENDING_THREADS,
    MAX_ZOMBIE_THREADS,
};

// ═══════════════════════════════════════════════════════════════
// Error Handling Exports
// ═══════════════════════════════════════════════════════════════

pub use error::{
    SchedulerError,
    SchedulerResult,
    QueueErrorType,
    ThreadStateError,
};

// ═══════════════════════════════════════════════════════════════
// Metrics Exports
// ═══════════════════════════════════════════════════════════════

pub use metrics::{
    SchedulerMetrics,
    MetricsSnapshot,
    METRICS,
};

// ═══════════════════════════════════════════════════════════════
// Policy Exports
// ═══════════════════════════════════════════════════════════════

pub use policy::{
    SchedulingPolicy,
    SchedParams,
    compare_priority,
    calculate_priority_boost,
    calculate_quantum_us,
};

// ═══════════════════════════════════════════════════════════════
// Load Balancing Exports
// ═══════════════════════════════════════════════════════════════

pub use loadbalancer::{
    LoadBalancer,
    CpuLoad,
    LoadImbalance,
    MigrationSuggestion,
    MigrationReason,
    LoadBalancerStats,
    LOAD_BALANCER,
    MAX_CPUS,
};

// ═══════════════════════════════════════════════════════════════
// Affinity Exports
// ═══════════════════════════════════════════════════════════════

pub use affinity::{CpuMask, ThreadAffinity};

// ═══════════════════════════════════════════════════════════════
// Prediction Exports
// ═══════════════════════════════════════════════════════════════

pub use statistics::SCHEDULER_STATS;
pub use predictive::PredictiveScheduler;

// ═══════════════════════════════════════════════════════════════
// V2 Legacy Exports (deprecated, for compatibility only)
// ═══════════════════════════════════════════════════════════════

/// Interrupt guard for critical sections
#[derive(Debug)]
pub struct InterruptGuard {
    _private: (),
}

impl InterruptGuard {
    /// Create new interrupt guard (disables interrupts)
    pub fn new() -> Self {
        unsafe { core::arch::asm!("cli", options(nomem, nostack)); }
        Self { _private: () }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)); }
    }
}
