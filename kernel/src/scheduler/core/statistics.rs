//! Statistics - Scheduler performance tracking
//!
//! Tracks context switches, latency, throughput, etc.

use core::sync::atomic::{AtomicU64, Ordering};

/// Global scheduler statistics
pub struct SchedulerStats {
    /// Total context switches
    pub total_switches: AtomicU64,
    
    /// Total threads created
    pub total_threads: AtomicU64,
    
    /// Total threads destroyed
    pub total_destroyed: AtomicU64,
    
    /// Total scheduling decisions (<100 cycles target)
    pub total_picks: AtomicU64,
    
    /// Total cycles spent in scheduler
    pub scheduler_cycles: AtomicU64,
    
    /// Minimum context switch time (cycles)
    pub min_switch_cycles: AtomicU64,
    
    /// Maximum context switch time (cycles)
    pub max_switch_cycles: AtomicU64,
    
    /// Total switch cycles (for average calculation)
    pub total_switch_cycles: AtomicU64,
    
    /// Preemptions (involuntary switches)
    pub preemptions: AtomicU64,
    
    /// Voluntary yields
    pub yields: AtomicU64,
    
    /// Idle cycles
    pub idle_cycles: AtomicU64,
}

impl SchedulerStats {
    pub const fn new() -> Self {
        Self {
            total_switches: AtomicU64::new(0),
            total_threads: AtomicU64::new(0),
            total_destroyed: AtomicU64::new(0),
            total_picks: AtomicU64::new(0),
            scheduler_cycles: AtomicU64::new(0),
            min_switch_cycles: AtomicU64::new(u64::MAX),
            max_switch_cycles: AtomicU64::new(0),
            total_switch_cycles: AtomicU64::new(0),
            preemptions: AtomicU64::new(0),
            yields: AtomicU64::new(0),
            idle_cycles: AtomicU64::new(0),
        }
    }
    
    /// Record context switch
    pub fn record_switch(&self, cycles: u64) {
        self.total_switches.fetch_add(1, Ordering::Relaxed);
        self.total_switch_cycles.fetch_add(cycles, Ordering::Relaxed);
        
        // Update min
        let mut current_min = self.min_switch_cycles.load(Ordering::Relaxed);
        while cycles < current_min {
            match self.min_switch_cycles.compare_exchange_weak(
                current_min,
                cycles,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_min = x,
            }
        }
        
        // Update max
        let mut current_max = self.max_switch_cycles.load(Ordering::Relaxed);
        while cycles > current_max {
            match self.max_switch_cycles.compare_exchange_weak(
                current_max,
                cycles,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }
    }
    
    /// Record scheduling decision
    pub fn record_pick(&self, cycles: u64) {
        self.total_picks.fetch_add(1, Ordering::Relaxed);
        self.scheduler_cycles.fetch_add(cycles, Ordering::Relaxed);
    }
    
    /// Record preemption
    pub fn record_preemption(&self) {
        self.preemptions.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Record voluntary yield
    pub fn record_yield(&self) {
        self.yields.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Record idle time
    pub fn record_idle(&self, cycles: u64) {
        self.idle_cycles.fetch_add(cycles, Ordering::Relaxed);
    }
    
    /// Get average switch time
    pub fn avg_switch_cycles(&self) -> u64 {
        let total = self.total_switch_cycles.load(Ordering::Relaxed);
        let count = self.total_switches.load(Ordering::Relaxed);
        if count > 0 {
            total / count
        } else {
            0
        }
    }
    
    /// Get average pick time
    pub fn avg_pick_cycles(&self) -> u64 {
        let total = self.scheduler_cycles.load(Ordering::Relaxed);
        let count = self.total_picks.load(Ordering::Relaxed);
        if count > 0 {
            total / count
        } else {
            0
        }
    }
    
    /// Get CPU utilization (percentage)
    pub fn cpu_utilization(&self) -> u8 {
        let idle = self.idle_cycles.load(Ordering::Relaxed);
        let total = self.total_switch_cycles.load(Ordering::Relaxed);
        
        if total > 0 {
            let busy = total.saturating_sub(idle);
            ((busy * 100) / total) as u8
        } else {
            0
        }
    }
}

/// Global statistics instance
pub static SCHEDULER_STATS: SchedulerStats = SchedulerStats::new();
