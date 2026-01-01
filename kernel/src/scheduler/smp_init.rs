//! SMP Scheduler Integration
//!
//! Integrates per-CPU schedulers with the SMP system

use crate::scheduler::per_cpu::SMP_SCHEDULER;
use crate::arch::x86_64::smp::SMP_SYSTEM;

/// Initialize SMP scheduler with detected CPUs
pub fn init_smp_scheduler() {
    let cpu_count = SMP_SYSTEM.cpu_count();
    
    if cpu_count > 1 {
        log::info!("[SMP Scheduler] Initializing for {} CPUs", cpu_count);
        SMP_SCHEDULER.set_cpu_count(cpu_count);
        
        // Log per-CPU scheduler setup
        for cpu_id in 0..cpu_count {
            if let Some(cpu_sched) = SMP_SCHEDULER.get_cpu_scheduler(cpu_id) {
                log::info!("[SMP Scheduler] CPU {} scheduler ready (queue: {})", 
                    cpu_id, cpu_sched.queue_length());
            }
        }
        
        log::info!("[SMP Scheduler] Load balancing enabled");
    } else {
        log::info!("[SMP Scheduler] Single CPU mode");
        SMP_SCHEDULER.set_cpu_count(1);
    }
}

/// Get the current CPU ID
/// Uses per-CPU data from GS segment (fast, single instruction)
pub fn current_cpu_id() -> usize {
    crate::arch::x86_64::percpu::cpu_id()
}

/// Schedule on current CPU
pub fn schedule_current_cpu() {
    let cpu_id = current_cpu_id();
    
    if let Some(cpu_sched) = SMP_SCHEDULER.get_cpu_scheduler(cpu_id) {
        // Pick next thread
        let next_thread = cpu_sched.pick_next();
        
        // Set as current
        cpu_sched.set_current(next_thread.clone());
        
        // TODO: Actually switch to the thread
        // This requires context switch integration
    }
}

/// Add a thread to the scheduler (load-balanced)
pub fn add_thread_smp(thread: alloc::sync::Arc<crate::scheduler::thread::Thread>) {
    SMP_SCHEDULER.add_thread(thread);
}

/// Try to steal work for current CPU if idle
pub fn try_steal_work_current() -> Option<alloc::sync::Arc<crate::scheduler::thread::Thread>> {
    let cpu_id = current_cpu_id();
    SMP_SCHEDULER.try_steal_work(cpu_id)
}

/// Print SMP scheduler statistics
pub fn print_smp_stats() {
    let stats = SMP_SCHEDULER.get_load_balance_stats();
    
    log::info!("[SMP Scheduler Stats]");
    log::info!("  CPUs: {}", stats.total_cpus);
    log::info!("  Total threads: {}", stats.total_threads);
    log::info!("  Avg load: {}", stats.avg_load);
    log::info!("  Max load: {}", stats.max_load);
    log::info!("  Min load: {}", stats.min_load);
    log::info!("  Imbalance: {:.2}%", stats.imbalance_ratio * 100.0);
    
    // Per-CPU stats
    for cpu_id in 0..stats.total_cpus {
        if let Some(cpu_sched) = SMP_SCHEDULER.get_cpu_scheduler(cpu_id) {
            let cpu_stats = cpu_sched.get_stats();
            log::info!("  CPU {}: switches={}, queue={}, stolen={}, migrated={}", 
                cpu_id,
                cpu_stats.context_switches,
                cpu_stats.queue_length,
                cpu_stats.threads_stolen,
                cpu_stats.threads_migrated
            );
        }
    }
}
