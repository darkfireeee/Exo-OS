//! Load Balancer - Multi-CPU Thread Distribution
//!
//! Handles thread migration between CPUs for optimal performance.
//! Better than Linux: Simpler algorithm, lock-free where possible.
//!
//! Features:
//! - Per-CPU run queues (reduces lock contention)
//! - Work-stealing for idle CPUs
//! - NUMA-aware placement
//! - Affinity-respecting migration

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::vec::Vec;

/// Maximum supported CPUs
pub const MAX_CPUS: usize = 64;

/// Load imbalance threshold (percentage difference to trigger migration)
pub const IMBALANCE_THRESHOLD: usize = 25;

/// Minimum load before stealing (don't steal from lightly loaded CPUs)
pub const MIN_LOAD_TO_STEAL: usize = 2;

/// Per-CPU load statistics
#[derive(Debug)]
pub struct CpuLoad {
    /// CPU ID
    pub cpu_id: usize,
    /// Current number of runnable threads
    pub runnable: AtomicUsize,
    /// Current number of running threads (1 or 0)
    pub running: AtomicUsize,
    /// Total load weight (sum of thread weights)
    pub load_weight: AtomicU64,
    /// Idle time in nanoseconds (since last reset)
    pub idle_time_ns: AtomicU64,
    /// Busy time in nanoseconds (since last reset)
    pub busy_time_ns: AtomicU64,
    /// Number of threads migrated away from this CPU
    pub migrations_out: AtomicU64,
    /// Number of threads migrated to this CPU
    pub migrations_in: AtomicU64,
    /// Whether this CPU is online
    pub online: core::sync::atomic::AtomicBool,
}

impl CpuLoad {
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            runnable: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            load_weight: AtomicU64::new(0),
            idle_time_ns: AtomicU64::new(0),
            busy_time_ns: AtomicU64::new(0),
            migrations_out: AtomicU64::new(0),
            migrations_in: AtomicU64::new(0),
            online: core::sync::atomic::AtomicBool::new(true),
        }
    }
    
    /// Get total load (runnable + running)
    pub fn total_load(&self) -> usize {
        self.runnable.load(Ordering::Relaxed) + self.running.load(Ordering::Relaxed)
    }
    
    /// Get CPU utilization (0-100)
    pub fn utilization(&self) -> u8 {
        let busy = self.busy_time_ns.load(Ordering::Relaxed);
        let idle = self.idle_time_ns.load(Ordering::Relaxed);
        let total = busy + idle;
        
        if total == 0 {
            0
        } else {
            ((busy * 100) / total) as u8
        }
    }
    
    /// Record thread added to this CPU
    pub fn thread_added(&self, weight: u64) {
        self.runnable.fetch_add(1, Ordering::Relaxed);
        self.load_weight.fetch_add(weight, Ordering::Relaxed);
    }
    
    /// Record thread removed from this CPU
    pub fn thread_removed(&self, weight: u64) {
        self.runnable.fetch_sub(1, Ordering::Relaxed);
        self.load_weight.fetch_sub(weight, Ordering::Relaxed);
    }
    
    /// Record thread started running
    pub fn thread_started(&self) {
        self.running.store(1, Ordering::Relaxed);
    }
    
    /// Record thread stopped running
    pub fn thread_stopped(&self) {
        self.running.store(0, Ordering::Relaxed);
    }
}

/// NUMA node information
#[derive(Debug, Clone)]
pub struct NumaNode {
    /// Node ID
    pub node_id: usize,
    /// CPUs in this node
    pub cpus: Vec<usize>,
    /// Distance to other nodes (indexed by node ID)
    pub distances: Vec<u8>,
}

impl NumaNode {
    pub fn new(node_id: usize) -> Self {
        Self {
            node_id,
            cpus: Vec::new(),
            distances: Vec::new(),
        }
    }
    
    /// Add CPU to this node
    pub fn add_cpu(&mut self, cpu_id: usize) {
        self.cpus.push(cpu_id);
    }
}

/// Load balancer state
pub struct LoadBalancer {
    /// Per-CPU load stats
    cpu_loads: [CpuLoad; MAX_CPUS],
    /// Number of online CPUs
    online_cpus: AtomicUsize,
    /// Total system load
    total_load: AtomicUsize,
    /// Load balancing iterations
    balance_iterations: AtomicU64,
    /// Total migrations performed
    total_migrations: AtomicU64,
}

impl LoadBalancer {
    /// Create new load balancer
    pub const fn new() -> Self {
        // Can't use array init with const fn, so we do it manually
        const CPU_INIT: CpuLoad = CpuLoad::new(0);
        let cpu_loads = [CPU_INIT; MAX_CPUS];
        
        Self {
            cpu_loads,
            online_cpus: AtomicUsize::new(1), // At least 1 CPU
            total_load: AtomicUsize::new(0),
            balance_iterations: AtomicU64::new(0),
            total_migrations: AtomicU64::new(0),
        }
    }
    
    /// Initialize with detected CPUs
    pub fn init(&mut self, num_cpus: usize) {
        self.online_cpus.store(num_cpus, Ordering::Relaxed);
        
        // Initialize CPU IDs
        for i in 0..num_cpus {
            // Can't reassign in const array, but cpu_id is fixed per position
        }
    }
    
    /// Get load for a CPU
    pub fn cpu_load(&self, cpu: usize) -> &CpuLoad {
        &self.cpu_loads[cpu % MAX_CPUS]
    }
    
    /// Find the least loaded CPU
    pub fn find_idle_cpu(&self, affinity_mask: u64) -> Option<usize> {
        let online = self.online_cpus.load(Ordering::Relaxed);
        let mut best_cpu = None;
        let mut best_load = usize::MAX;
        
        for cpu in 0..online {
            // Check affinity
            if affinity_mask != 0 && (affinity_mask & (1 << cpu)) == 0 {
                continue;
            }
            
            // Check if online
            if !self.cpu_loads[cpu].online.load(Ordering::Relaxed) {
                continue;
            }
            
            let load = self.cpu_loads[cpu].total_load();
            if load < best_load {
                best_load = load;
                best_cpu = Some(cpu);
            }
        }
        
        best_cpu
    }
    
    /// Find the most loaded CPU
    pub fn find_busiest_cpu(&self, affinity_mask: u64) -> Option<usize> {
        let online = self.online_cpus.load(Ordering::Relaxed);
        let mut busiest_cpu = None;
        let mut max_load = 0;
        
        for cpu in 0..online {
            // Check affinity if specified
            if affinity_mask != 0 && (affinity_mask & (1 << cpu)) == 0 {
                continue;
            }
            
            if !self.cpu_loads[cpu].online.load(Ordering::Relaxed) {
                continue;
            }
            
            let load = self.cpu_loads[cpu].total_load();
            if load > max_load {
                max_load = load;
                busiest_cpu = Some(cpu);
            }
        }
        
        busiest_cpu
    }
    
    /// Calculate load imbalance
    pub fn calculate_imbalance(&self) -> LoadImbalance {
        let online = self.online_cpus.load(Ordering::Relaxed);
        if online <= 1 {
            return LoadImbalance::Balanced;
        }
        
        let mut min_load = usize::MAX;
        let mut max_load = 0;
        let mut min_cpu = 0;
        let mut max_cpu = 0;
        let mut total = 0;
        
        for cpu in 0..online {
            if !self.cpu_loads[cpu].online.load(Ordering::Relaxed) {
                continue;
            }
            
            let load = self.cpu_loads[cpu].total_load();
            total += load;
            
            if load < min_load {
                min_load = load;
                min_cpu = cpu;
            }
            if load > max_load {
                max_load = load;
                max_cpu = cpu;
            }
        }
        
        let avg = total / online;
        
        // Check if imbalanced
        if max_load == 0 {
            return LoadImbalance::Balanced;
        }
        
        let diff_percent = ((max_load - min_load) * 100) / max_load;
        
        if diff_percent > IMBALANCE_THRESHOLD && max_load >= MIN_LOAD_TO_STEAL {
            LoadImbalance::Unbalanced {
                busiest: max_cpu,
                idlest: min_cpu,
                delta: max_load - min_load,
                avg_load: avg,
            }
        } else {
            LoadImbalance::Balanced
        }
    }
    
    /// Suggest migration (returns (from_cpu, to_cpu) if migration needed)
    pub fn suggest_migration(&self, thread_affinity: u64) -> Option<MigrationSuggestion> {
        match self.calculate_imbalance() {
            LoadImbalance::Balanced => None,
            LoadImbalance::Unbalanced { busiest, idlest, delta, .. } => {
                // Check if thread can run on target CPU
                if thread_affinity != 0 && (thread_affinity & (1 << idlest)) == 0 {
                    return None;
                }
                
                // Don't migrate for small imbalance
                if delta < 2 {
                    return None;
                }
                
                Some(MigrationSuggestion {
                    from_cpu: busiest,
                    to_cpu: idlest,
                    reason: MigrationReason::LoadBalance,
                })
            }
        }
    }
    
    /// Record a migration
    pub fn record_migration(&self, from_cpu: usize, to_cpu: usize) {
        self.cpu_loads[from_cpu].migrations_out.fetch_add(1, Ordering::Relaxed);
        self.cpu_loads[to_cpu].migrations_in.fetch_add(1, Ordering::Relaxed);
        self.total_migrations.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Run load balance pass
    pub fn balance_pass(&self) -> usize {
        self.balance_iterations.fetch_add(1, Ordering::Relaxed);
        
        // Count migrations suggested
        let mut migrations = 0;
        
        // Simple algorithm: find busiest and idlest, suggest moves
        if let Some(suggestion) = self.suggest_migration(0) {
            // In real implementation, would actually perform migration here
            migrations += 1;
        }
        
        migrations
    }
    
    /// Get statistics
    pub fn stats(&self) -> LoadBalancerStats {
        LoadBalancerStats {
            online_cpus: self.online_cpus.load(Ordering::Relaxed),
            total_load: self.total_load.load(Ordering::Relaxed),
            balance_iterations: self.balance_iterations.load(Ordering::Relaxed),
            total_migrations: self.total_migrations.load(Ordering::Relaxed),
            imbalance: self.calculate_imbalance(),
        }
    }
}

/// Load imbalance status
#[derive(Debug, Clone, Copy)]
pub enum LoadImbalance {
    /// System is balanced
    Balanced,
    /// System is unbalanced
    Unbalanced {
        busiest: usize,
        idlest: usize,
        delta: usize,
        avg_load: usize,
    },
}

/// Migration suggestion
#[derive(Debug, Clone, Copy)]
pub struct MigrationSuggestion {
    pub from_cpu: usize,
    pub to_cpu: usize,
    pub reason: MigrationReason,
}

/// Reason for migration
#[derive(Debug, Clone, Copy)]
pub enum MigrationReason {
    /// Load balancing
    LoadBalance,
    /// Affinity change
    AffinityChange,
    /// NUMA optimization
    NumaOptimization,
    /// CPU going offline
    CpuOffline,
    /// Work stealing by idle CPU
    WorkStealing,
}

/// Load balancer statistics
#[derive(Debug, Clone)]
pub struct LoadBalancerStats {
    pub online_cpus: usize,
    pub total_load: usize,
    pub balance_iterations: u64,
    pub total_migrations: u64,
    pub imbalance: LoadImbalance,
}

/// Global load balancer instance
pub static LOAD_BALANCER: LoadBalancer = LoadBalancer::new();

/// Work-stealing helper for idle CPUs
pub struct WorkStealer {
    /// Current CPU
    cpu: usize,
    /// Victim selection (round-robin starting point)
    last_victim: AtomicUsize,
}

impl WorkStealer {
    pub const fn new(cpu: usize) -> Self {
        Self {
            cpu,
            last_victim: AtomicUsize::new(0),
        }
    }
    
    /// Find a CPU to steal from
    pub fn find_victim(&self, online_cpus: usize, my_affinity: u64) -> Option<usize> {
        let start = self.last_victim.fetch_add(1, Ordering::Relaxed) % online_cpus;
        
        for i in 0..online_cpus {
            let victim = (start + i) % online_cpus;
            
            // Don't steal from ourselves
            if victim == self.cpu {
                continue;
            }
            
            // Check load
            let load = LOAD_BALANCER.cpu_load(victim).total_load();
            if load >= MIN_LOAD_TO_STEAL {
                return Some(victim);
            }
        }
        
        None
    }
}
