//! Per-CPU Scheduler for SMP Support
//!
//! Each CPU has its own run queue and scheduler state.
//! Load balancing distributes threads across CPUs.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use super::thread::{Thread, ThreadId};

/// Maximum number of CPUs supported
pub const MAX_CPUS: usize = 32;

/// Per-CPU Scheduler State
pub struct PerCpuScheduler {
    /// CPU ID
    cpu_id: usize,
    
    /// Local run queue (threads ready to run on this CPU)
    run_queue: Mutex<VecDeque<Arc<Thread>>>,
    
    /// Currently running thread
    current: Mutex<Option<Arc<Thread>>>,
    
    /// Idle thread for this CPU
    idle_thread: Option<Arc<Thread>>,
    
    /// Statistics
    stats: CpuStats,
}

/// Per-CPU Statistics
pub struct CpuStats {
    /// Number of context switches on this CPU
    pub context_switches: AtomicU64,
    
    /// Time spent idle (in ticks)
    pub idle_time: AtomicU64,
    
    /// Time spent running (in ticks)
    pub busy_time: AtomicU64,
    
    /// Number of threads currently in run queue
    pub queue_length: AtomicUsize,
    
    /// Number of threads stolen from other CPUs
    pub threads_stolen: AtomicU64,
    
    /// Number of threads migrated to other CPUs
    pub threads_migrated: AtomicU64,
}

impl PerCpuScheduler {
    /// Create a new per-CPU scheduler
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            run_queue: Mutex::new(VecDeque::new()),
            current: Mutex::new(None),
            idle_thread: None,
            stats: CpuStats {
                context_switches: AtomicU64::new(0),
                idle_time: AtomicU64::new(0),
                busy_time: AtomicU64::new(0),
                queue_length: AtomicUsize::new(0),
                threads_stolen: AtomicU64::new(0),
                threads_migrated: AtomicU64::new(0),
            },
        }
    }
    
    /// Initialize with an idle thread
    pub fn init(&mut self, idle_thread: Arc<Thread>) {
        self.idle_thread = Some(idle_thread);
    }
    
    /// Add a thread to this CPU's run queue
    pub fn add_thread(&self, thread: Arc<Thread>) {
        let mut queue = self.run_queue.lock();
        queue.push_back(thread);
        self.stats.queue_length.store(queue.len(), Ordering::Relaxed);
    }
    
    /// Pick the next thread to run
    /// Returns the idle thread if queue is empty
    pub fn pick_next(&self) -> Arc<Thread> {
        let mut queue = self.run_queue.lock();
        
        if let Some(thread) = queue.pop_front() {
            self.stats.queue_length.store(queue.len(), Ordering::Relaxed);
            self.stats.context_switches.fetch_add(1, Ordering::Relaxed);
            thread
        } else {
            // No threads ready, return idle
            self.idle_thread.as_ref()
                .expect("Idle thread not initialized")
                .clone()
        }
    }
    
    /// Set the currently running thread
    pub fn set_current(&self, thread: Arc<Thread>) {
        *self.current.lock() = Some(thread);
    }
    
    /// Get the currently running thread
    pub fn get_current(&self) -> Option<Arc<Thread>> {
        self.current.lock().clone()
    }
    
    /// Get the number of threads in the run queue
    pub fn queue_length(&self) -> usize {
        self.run_queue.lock().len()
    }
    
    /// Get CPU load (queue length + running thread)
    pub fn get_load(&self) -> usize {
        let queue_len = self.queue_length();
        let running = if self.current.lock().is_some() { 1 } else { 0 };
        queue_len + running
    }
    
    /// Try to steal a thread from this CPU's queue
    /// Used by work stealing algorithm
    pub fn try_steal(&self) -> Option<Arc<Thread>> {
        let mut queue = self.run_queue.lock();
        
        // Only steal if we have more than 2 threads
        if queue.len() > 2 {
            let thread = queue.pop_back()?; // Steal from back
            self.stats.queue_length.store(queue.len(), Ordering::Relaxed);
            self.stats.threads_migrated.fetch_add(1, Ordering::Relaxed);
            Some(thread)
        } else {
            None
        }
    }
    
    /// Get statistics for this CPU
    pub fn get_stats(&self) -> CpuStatsSnapshot {
        CpuStatsSnapshot {
            cpu_id: self.cpu_id,
            context_switches: self.stats.context_switches.load(Ordering::Relaxed),
            idle_time: self.stats.idle_time.load(Ordering::Relaxed),
            busy_time: self.stats.busy_time.load(Ordering::Relaxed),
            queue_length: self.stats.queue_length.load(Ordering::Relaxed),
            threads_stolen: self.stats.threads_stolen.load(Ordering::Relaxed),
            threads_migrated: self.stats.threads_migrated.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of CPU statistics
#[derive(Debug, Clone, Copy)]
pub struct CpuStatsSnapshot {
    pub cpu_id: usize,
    pub context_switches: u64,
    pub idle_time: u64,
    pub busy_time: u64,
    pub queue_length: usize,
    pub threads_stolen: u64,
    pub threads_migrated: u64,
}

/// Global SMP Scheduler
pub struct SmpScheduler {
    /// Per-CPU schedulers
    cpu_schedulers: [PerCpuScheduler; MAX_CPUS],
    
    /// Number of active CPUs
    cpu_count: AtomicUsize,
}

impl SmpScheduler {
    /// Create a new SMP scheduler
    pub const fn new() -> Self {
        // Use macro to initialize array without const evaluation issues
        const fn make_scheduler(id: usize) -> PerCpuScheduler {
            PerCpuScheduler::new(id)
        }
        
        Self {
            cpu_schedulers: [
                make_scheduler(0), make_scheduler(1), make_scheduler(2), make_scheduler(3),
                make_scheduler(4), make_scheduler(5), make_scheduler(6), make_scheduler(7),
                make_scheduler(8), make_scheduler(9), make_scheduler(10), make_scheduler(11),
                make_scheduler(12), make_scheduler(13), make_scheduler(14), make_scheduler(15),
                make_scheduler(16), make_scheduler(17), make_scheduler(18), make_scheduler(19),
                make_scheduler(20), make_scheduler(21), make_scheduler(22), make_scheduler(23),
                make_scheduler(24), make_scheduler(25), make_scheduler(26), make_scheduler(27),
                make_scheduler(28), make_scheduler(29), make_scheduler(30), make_scheduler(31),
            ],
            cpu_count: AtomicUsize::new(1), // Start with BSP
        }
    }
    
    /// Set the number of active CPUs
    pub fn set_cpu_count(&self, count: usize) {
        self.cpu_count.store(count.min(MAX_CPUS), Ordering::Release);
    }
    
    /// Get the number of active CPUs
    pub fn get_cpu_count(&self) -> usize {
        self.cpu_count.load(Ordering::Acquire)
    }
    
    /// Get a per-CPU scheduler
    pub fn get_cpu_scheduler(&self, cpu_id: usize) -> Option<&PerCpuScheduler> {
        if cpu_id < MAX_CPUS {
            Some(&self.cpu_schedulers[cpu_id])
        } else {
            None
        }
    }
    
    /// Add a thread to the least loaded CPU
    pub fn add_thread(&self, thread: Arc<Thread>) {
        let target_cpu = self.choose_cpu_for_thread(&thread);
        
        if let Some(cpu_sched) = self.get_cpu_scheduler(target_cpu) {
            cpu_sched.add_thread(thread);
        }
    }
    
    /// Choose the best CPU for a new thread
    /// Strategy: Least loaded CPU
    fn choose_cpu_for_thread(&self, _thread: &Thread) -> usize {
        let cpu_count = self.get_cpu_count();
        let mut min_load = usize::MAX;
        let mut target_cpu = 0;
        
        for cpu_id in 0..cpu_count {
            if let Some(cpu_sched) = self.get_cpu_scheduler(cpu_id) {
                let load = cpu_sched.get_load();
                if load < min_load {
                    min_load = load;
                    target_cpu = cpu_id;
                }
            }
        }
        
        target_cpu
    }
    
    /// Work stealing: Try to steal work for an idle CPU
    pub fn try_steal_work(&self, idle_cpu: usize) -> Option<Arc<Thread>> {
        let cpu_count = self.get_cpu_count();
        
        // Find the most loaded CPU
        let mut max_load = 0;
        let mut victim_cpu = None;
        
        for cpu_id in 0..cpu_count {
            if cpu_id == idle_cpu {
                continue; // Don't steal from ourselves
            }
            
            if let Some(cpu_sched) = self.get_cpu_scheduler(cpu_id) {
                let load = cpu_sched.get_load();
                if load > max_load {
                    max_load = load;
                    victim_cpu = Some(cpu_id);
                }
            }
        }
        
        // Try to steal from the victim
        if let Some(victim_id) = victim_cpu {
            if let Some(victim_sched) = self.get_cpu_scheduler(victim_id) {
                if let Some(thread) = victim_sched.try_steal() {
                    // Update statistics
                    if let Some(idle_sched) = self.get_cpu_scheduler(idle_cpu) {
                        idle_sched.stats.threads_stolen.fetch_add(1, Ordering::Relaxed);
                    }
                    return Some(thread);
                }
            }
        }
        
        None
    }
    
    /// Get load balancing statistics
    pub fn get_load_balance_stats(&self) -> LoadBalanceStats {
        let cpu_count = self.get_cpu_count();
        let mut total_load = 0;
        let mut max_load = 0;
        let mut min_load = usize::MAX;
        
        for cpu_id in 0..cpu_count {
            if let Some(cpu_sched) = self.get_cpu_scheduler(cpu_id) {
                let load = cpu_sched.get_load();
                total_load += load;
                max_load = max_load.max(load);
                min_load = min_load.min(load);
            }
        }
        
        let avg_load = if cpu_count > 0 {
            total_load / cpu_count
        } else {
            0
        };
        
        let imbalance = if min_load > 0 {
            (max_load as f32 / min_load as f32) - 1.0
        } else {
            0.0
        };
        
        LoadBalanceStats {
            total_cpus: cpu_count,
            total_threads: total_load,
            avg_load,
            max_load,
            min_load,
            imbalance_ratio: imbalance,
        }
    }
}

/// Load balancing statistics
#[derive(Debug, Clone, Copy)]
pub struct LoadBalanceStats {
    pub total_cpus: usize,
    pub total_threads: usize,
    pub avg_load: usize,
    pub max_load: usize,
    pub min_load: usize,
    pub imbalance_ratio: f32, // 0.0 = perfectly balanced
}

/// Global SMP scheduler instance
pub static SMP_SCHEDULER: SmpScheduler = SmpScheduler::new();
