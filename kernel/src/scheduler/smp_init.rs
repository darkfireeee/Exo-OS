//! SMP Scheduler Integration
//!
//! Integrates per-CPU schedulers with the SMP system

use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
use crate::arch::x86_64::smp::SMP_SYSTEM;

/// Initialize SMP scheduler with detected CPUs
pub fn init_smp_scheduler() {
    let cpu_count = SMP_SYSTEM.cpu_count();
    
    log::info!("[SMP Scheduler] Initializing for {} CPUs", cpu_count);
    
    // Initialize per-CPU queues
    crate::scheduler::core::percpu_queue::init();
    
    if cpu_count > 1 {
        // Create idle threads for each CPU
        log::info!("[SMP Scheduler] Creating idle threads...");
        for cpu_id in 0..cpu_count {
            let idle_thread = alloc::sync::Arc::new(
                crate::scheduler::idle::create_idle_thread_for_cpu(cpu_id as u32)
            );
            
            if let Some(queue) = PER_CPU_QUEUES.get(cpu_id) {
                queue.set_current_thread(Some(idle_thread));
                log::info!("[SMP Scheduler] CPU {} idle thread ready", cpu_id);
            }
        }
        
        log::info!("[SMP Scheduler] Load balancing enabled");
    } else {
        log::info!("[SMP Scheduler] Single CPU mode");
        
        // Create idle thread for BSP
        let idle_thread = alloc::sync::Arc::new(
            crate::scheduler::idle::create_idle_thread_for_cpu(0)
        );
        
        if let Some(queue) = PER_CPU_QUEUES.get(0) {
            queue.set_current_thread(Some(idle_thread));
            log::info!("[SMP Scheduler] BSP idle thread ready");
        }
    }
}

/// Get the current CPU ID
/// Uses per-CPU data from GS segment (fast, single instruction)
#[inline]
pub fn current_cpu_id() -> usize {
    crate::arch::x86_64::percpu::cpu_id()
}
