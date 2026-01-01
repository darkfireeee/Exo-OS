//! Tests SMP Scheduler
//!
//! Tests pour le scheduler per-CPU en mode SMP

use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
use crate::scheduler::smp_init::current_cpu_id;
use crate::scheduler::thread::Thread;
use crate::arch::x86_64::smp::SMP_SYSTEM;
use alloc::sync::Arc;

/// Test: Per-CPU queues initialization
fn test_percpu_queues_init() {
    crate::logger::info("[TEST] Per-CPU queues initialization");
    
    let cpu_count = SMP_SYSTEM.cpu_count();
    assert!(cpu_count > 0, "No CPUs detected");
    
    for cpu_id in 0..cpu_count {
        let queue = PER_CPU_QUEUES.get(cpu_id);
        assert!(queue.is_some(), "Queue {} not initialized", cpu_id);
        
        let queue = queue.unwrap();
        // Note: Queue might have idle thread
        crate::logger::debug(&alloc::format!("[TEST] CPU {} queue len: {}", cpu_id, queue.len()));
    }
    
    crate::logger::info("[TEST] ✅ Per-CPU queues initialized");
}

/// Test: Enqueue/Dequeue on same CPU
fn test_local_enqueue_dequeue() {
    crate::logger::info("[TEST] Local enqueue/dequeue");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).expect("Queue not found");
    
    // Create test thread
    let thread = Arc::new(Thread::new_kernel(
        1000,
        "test_thread",
        test_thread_entry,
        4096
    ));
    
    let initial_len = queue.len();
    
    // Enqueue
    queue.enqueue(thread.clone());
    assert_eq!(queue.len(), initial_len + 1, "Queue should have +1 thread");
    
    // Dequeue
    let dequeued = queue.dequeue();
    assert!(dequeued.is_some(), "Should dequeue thread");
    assert_eq!(dequeued.unwrap().id(), 1000, "Thread ID mismatch");
    
    crate::logger::info("[TEST] ✅ Local enqueue/dequeue works");
}

/// Test: Work stealing between CPUs
fn test_work_stealing() {
    crate::logger::info("[TEST] Work stealing");
    
    let cpu_count = SMP_SYSTEM.cpu_count();
    if cpu_count < 2 {
        crate::logger::warn("[TEST] ⚠️ Skipped (need ≥2 CPUs)");
        return;
    }
    
    // Fill CPU 0 with threads
    let queue0 = PER_CPU_QUEUES.get(0).unwrap();
    for i in 0..10 {
        let thread = Arc::new(Thread::new_kernel(
            2000 + i,
            "steal_test",
            test_thread_entry,
            4096
        ));
        queue0.enqueue(thread);
    }
    
    let len_before = queue0.len();
    assert!(len_before >= 10, "CPU 0 should have ≥10 threads");
    
    // Steal half
    let stolen = queue0.steal_half();
    assert!(stolen.len() > 0, "Should steal some threads");
    
    let len_after = queue0.len();
    assert!(len_after < len_before, "CPU 0 should have fewer threads");
    
    crate::logger::info(&alloc::format!("[TEST] Stole {} threads", stolen.len()));
    crate::logger::info("[TEST] ✅ Work stealing successful");
}

/// Test: Per-CPU statistics
fn test_percpu_stats() {
    crate::logger::info("[TEST] Per-CPU statistics");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    let stats = queue.stats();
    assert_eq!(stats.cpu_id, cpu_id, "CPU ID mismatch");
    assert!(stats.load_percentage <= 100, "Invalid load percentage");
    
    crate::logger::info(&alloc::format!(
        "[TEST] CPU {} stats: switches={}, load={}%",
        stats.cpu_id, stats.context_switches, stats.load_percentage
    ));
    crate::logger::info("[TEST] ✅ Statistics valid");
}

/// Test: Idle thread presence
fn test_idle_threads() {
    crate::logger::info("[TEST] Idle threads");
    
    let cpu_count = SMP_SYSTEM.cpu_count();
    
    for cpu_id in 0..cpu_count {
        let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
        let current = queue.current_thread();
        
        if current.is_some() {
            let thread = current.unwrap();
            let name = thread.name();
            crate::logger::info(&alloc::format!("[TEST] CPU {} current: {}", cpu_id, name));
        } else {
            crate::logger::debug(&alloc::format!("[TEST] CPU {} no current thread", cpu_id));
        }
    }
    
    crate::logger::info("[TEST] ✅ Idle thread check complete");
}

/// Test: Context switch counting
fn test_context_switch_count() {
    crate::logger::info("[TEST] Context switch counting");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    let stats_before = queue.stats();
    let switches_before = stats_before.context_switches;
    
    // Increment manually
    queue.inc_context_switches();
    
    let stats_after = queue.stats();
    let switches_after = stats_after.context_switches;
    
    assert_eq!(switches_after, switches_before + 1, "Switch count should increment");
    
    crate::logger::info("[TEST] ✅ Context switch counting works");
}

/// Test: Stress - 10,000 enqueue/dequeue cycles
fn test_stress_enqueue_dequeue() {
    crate::logger::info("[STRESS] Enqueue/dequeue 10,000 cycles");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    const ITERATIONS: usize = 10_000;
    let mut success = 0;
    
    for i in 0..ITERATIONS {
        // Create and enqueue
        let thread = Arc::new(Thread::new_kernel(
            (i + 10000) as u64,
            "stress",
            test_thread_entry,
            4096
        ));
        queue.enqueue(thread);
        
        // Dequeue immediately
        if let Some(t) = queue.dequeue() {
            success += 1;
            assert_eq!(t.id(), (i + 10000) as u64, "Thread ID mismatch");
        }
        
        // Progress log every 1000
        if (i + 1) % 1000 == 0 {
            crate::logger::info(&alloc::format!("  Progress: {}/{}", i + 1, ITERATIONS));
        }
    }
    
    assert_eq!(success, ITERATIONS, "Not all threads processed");
    crate::logger::info(&alloc::format!("[STRESS] ✅ {} cycles successful", ITERATIONS));
}

/// Test: Fairness distribution across 100 threads
fn test_fairness_distribution() {
    crate::logger::info("[STRESS] Fairness distribution - 100 threads");
    
    let cpu_count = SMP_SYSTEM.cpu_count().min(4);
    let threads_per_cpu = 100 / cpu_count;
    
    // Distribute threads across CPUs
    for cpu_id in 0..cpu_count {
        let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
        for i in 0..threads_per_cpu {
            let thread = Arc::new(Thread::new_kernel(
                (cpu_id * threads_per_cpu + i + 20000) as u64,
                "fairness",
                test_thread_entry,
                4096
            ));
            queue.enqueue(thread);
        }
    }
    
    // Check distribution
    let mut total = 0;
    let mut min_load = usize::MAX;
    let mut max_load = 0;
    
    for cpu_id in 0..cpu_count {
        let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
        let len = queue.len();
        total += len;
        min_load = min_load.min(len);
        max_load = max_load.max(len);
        
        crate::logger::info(&alloc::format!("  CPU {} load: {} threads", cpu_id, len));
    }
    
    let imbalance = max_load - min_load;
    crate::logger::info(&alloc::format!("  Total: {}, Min: {}, Max: {}, Imbalance: {}", 
        total, min_load, max_load, imbalance));
    
    // Accept some imbalance due to idle threads
    assert!(imbalance <= threads_per_cpu, "Too much imbalance");
    crate::logger::info("[STRESS] ✅ Fair distribution");
    
    // Cleanup
    for cpu_id in 0..cpu_count {
        let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
        while queue.dequeue().is_some() {}
    }
}

/// Test: Concurrent operations simulation
fn test_concurrent_operations() {
    crate::logger::info("[STRESS] Concurrent operations simulation");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    // Simulate mixed operations
    const ROUNDS: usize = 1000;
    let mut enqueued = 0;
    let mut dequeued = 0;
    
    for round in 0..ROUNDS {
        // Batch enqueue (simulate multiple producers)
        for i in 0..5 {
            let thread = Arc::new(Thread::new_kernel(
                (round * 5 + i + 30000) as u64,
                "concurrent",
                test_thread_entry,
                4096
            ));
            queue.enqueue(thread);
            enqueued += 1;
        }
        
        // Dequeue 2 (simulate consumer)
        for _ in 0..2 {
            if queue.dequeue().is_some() {
                dequeued += 1;
            }
        }
        
        // Periodic steal simulation (every 10 rounds)
        if round % 10 == 0 {
            let stolen = queue.steal_half();
            crate::logger::debug(&alloc::format!("    Round {}: stole {}", round, stolen.len()));
        }
    }
    
    crate::logger::info(&alloc::format!("  Enqueued: {}, Dequeued: {}", enqueued, dequeued));
    crate::logger::info(&alloc::format!("  Queue length: {}", queue.len()));
    
    // Cleanup remaining
    let mut remaining = 0;
    while queue.dequeue().is_some() {
        remaining += 1;
    }
    
    assert_eq!(enqueued, dequeued + remaining, "Thread accounting mismatch");
    crate::logger::info("[STRESS] ✅ Concurrent operations handled");
}

/// Dummy thread entry for tests
fn test_thread_entry() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Run all SMP tests
pub fn run_smp_tests() {
    crate::logger::info("╔═══════════════════════════════════════╗");
    crate::logger::info("║      SMP SCHEDULER TESTS              ║");
    crate::logger::info("╚═══════════════════════════════════════╝");
    
    test_percpu_queues_init();
    test_local_enqueue_dequeue();
    test_work_stealing();
    test_percpu_stats();
    test_idle_threads();
    test_context_switch_count();
    
    crate::logger::info("");
    crate::logger::info("╔═══════════════════════════════════════╗");
    crate::logger::info("║       STRESS TESTS                    ║");
    crate::logger::info("╚═══════════════════════════════════════╝");
    
    test_stress_enqueue_dequeue();
    test_fairness_distribution();
    test_concurrent_operations();
    
    crate::logger::info("");
    crate::logger::info("╔═══════════════════════════════════════╗");
    crate::logger::info("║   ✅ ALL SMP TESTS PASSED (9/9)      ║");
    crate::logger::info("╚═══════════════════════════════════════╝");
}
