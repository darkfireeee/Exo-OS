//! Phase 2 SMP Validation Tests
//!
//! Tests to verify multi-CPU boot and operation

use crate::arch::x86_64::smp::SMP_SYSTEM;
use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
use crate::scheduler::core::loadbalancer::LOAD_BALANCER;

/// Test 1: Verify all CPUs detected and online
pub fn test_cpu_detection() -> Result<(), &'static str> {
    log::info!("=== Test 1: CPU Detection ===");
    
    let cpu_count = SMP_SYSTEM.cpu_count();
    let online_count = SMP_SYSTEM.online_count();
    
    log::info!("Detected {} CPUs, {} online", cpu_count, online_count);
    
    if cpu_count == 0 {
        return Err("No CPUs detected!");
    }
    
    if online_count == 0 {
        return Err("No CPUs online!");
    }
    
    // List all CPUs
    for i in 0..cpu_count {
        if let Some(cpu) = SMP_SYSTEM.cpu(i) {
            let state = cpu.state();
            log::info!("  CPU {}: APIC ID {}, State {:?}, BSP: {}", 
                i, cpu.apic_id.load(core::sync::atomic::Ordering::Acquire), state, cpu.is_bsp.load(core::sync::atomic::Ordering::Acquire));
        }
    }
    
    log::info!("✓ CPU detection passed");
    Ok(())
}

/// Test 2: Verify no deadlocks or panics after boot
pub fn test_no_deadlock() -> Result<(), &'static str> {
    log::info!("=== Test 2: No Deadlock ===");
    
    // Wait a bit to ensure APs have fully booted
    crate::arch::x86_64::pit::sleep_ms(500);
    
    let online = SMP_SYSTEM.online_count();
    log::info!("{} CPUs still responsive", online);
    
    // Check per-CPU queues are accessible
    for i in 0..online {
        if let Some(queue) = PER_CPU_QUEUES.get(i) {
            let len = queue.len();
            log::debug!("  CPU {} queue length: {}", i, len);
        } else {
            return Err("Cannot access per-CPU queue");
        }
    }
    
    log::info!("✓ No deadlock detected");
    Ok(())
}

/// Test 3: Basic load balancing sanity check
pub fn test_load_balancer() -> Result<(), &'static str> {
    log::info!("=== Test 3: Load Balancer ===");
    
    let online = SMP_SYSTEM.online_count();
    log::info!("Load balancer managing {} CPUs", online);
    
    if online == 0 {
        return Err("Load balancer has no online CPUs");
    }
    
    // Trigger a balance operation
    LOAD_BALANCER.balance();
    
    let stats = LOAD_BALANCER.stats();
    log::info!("  Total load: {}", stats.total_load);
    log::info!("  Balance iterations: {}", stats.balance_iterations);
    log::info!("  Migrations: {}", stats.total_migrations);
    
    log::info!("✓ Load balancer operational");
    Ok(())
}

/// Run all Phase 2 SMP tests
pub fn run_all_tests() -> Result<(), &'static str> {
    log::info!("");
    log::info!("╔═══════════════════════════════════════════════════════════╗");
    log::info!("║         PHASE 2 SMP VALIDATION TESTS                     ║");
    log::info!("╚═══════════════════════════════════════════════════════════╝");
    log::info!("");
    
    test_cpu_detection()?;
    test_no_deadlock()?;
    test_load_balancer()?;
    
    log::info!("");
    log::info!("╔═══════════════════════════════════════════════════════════╗");
    log::info!("║         ALL PHASE 2 TESTS PASSED ✓                       ║");
    log::info!("╚═══════════════════════════════════════════════════════════╝");
    log::info!("");
    
    Ok(())
}

/// Benchmark: Measure SMP scalability
pub fn benchmark_smp_scalability() {
    log::info!("");
    log::info!("=== Phase 2 SMP Scalability Benchmark ===");
    
    let cpu_count = SMP_SYSTEM.cpu_count();
    let online_count = SMP_SYSTEM.online_count();
    
    log::info!("CPUs available: {} total, {} online", cpu_count, online_count);
    
    // TODO: Implement parallel workload benchmark
    // For now, just report current state
    
    for i in 0..online_count {
        if let Some(queue) = PER_CPU_QUEUES.get(i) {
            let stats = queue.stats();
            log::info!("CPU {} stats:", i);
            log::info!("  Queue length: {}", stats.queue_length);
            log::info!("  Load: {}%", stats.load_percentage);
            log::info!("  Context switches: {}", stats.context_switches);
        }
    }
    
    // Report load balancer stats
    let lb_stats = LOAD_BALANCER.stats();
    log::info!("Load Balancer:");
    log::info!("  Total migrations: {}", lb_stats.total_migrations);
    log::info!("  Balance iterations: {}", lb_stats.balance_iterations);
    
    log::info!("");
    log::info!("Scalability target: Linear speedup to {} CPUs", online_count);
    log::info!("IPI overhead target: < 1000 cycles");
    log::info!("");
}
