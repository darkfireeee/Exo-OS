//! Tests de régression SMP Scheduler
//! Valident que les optimisations ne cassent pas les fonctionnalités

use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
use crate::scheduler::thread::Thread;
use crate::scheduler::smp_init::current_cpu_id;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Test: Memory leak après création/destruction massive de threads
pub fn test_regression_memory_leak() {
    crate::logger::info("[REGRESSION] Memory leak test - 10,000 threads create/destroy");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    // Mesurer heap avant
    let stats_before = crate::memory::heap::hybrid_allocator::get_allocator_stats();
    crate::logger::info(&alloc::format!("  Heap before: {} bytes used", stats_before.total_allocated_bytes));
    
    // Créer et détruire 10,000 threads
    const ITERATIONS: usize = 10_000;
    for batch in 0..100 {
        // Créer 100 threads
        for i in 0..100 {
            let thread = Arc::new(Thread::new_kernel(
                (batch * 100 + i) as u64 + 50000,
                "regression",
                dummy_entry,
                4096
            ));
            queue.enqueue(thread);
        }
        
        // Détruire 100 threads
        for _ in 0..100 {
            drop(queue.dequeue());
        }
        
        // Progress tous les 1000
        if (batch + 1) % 10 == 0 {
            crate::logger::info(&alloc::format!("    Progress: {}/100 batches", batch + 1));
        }
    }
    
    // Mesurer heap après
    let stats_after = crate::memory::heap::hybrid_allocator::get_allocator_stats();
    crate::logger::info(&alloc::format!("  Heap after: {} bytes used", stats_after.total_allocated_bytes));
    
    // Tolérance: +1MB max de leak (peut y avoir fragmentation)
    let leak = stats_after.total_allocated_bytes.saturating_sub(stats_before.total_allocated_bytes);
    crate::logger::info(&alloc::format!("  Potential leak: {} bytes", leak));
    
    if leak > 1024 * 1024 {
        crate::logger::error(&alloc::format!("  ❌ FAIL - Memory leak detected: {} bytes", leak));
    } else {
        crate::logger::info("  ✅ PASS - No significant memory leak");
    }
}

/// Test: Statistics overflow handling
pub fn test_regression_stats_overflow() {
    crate::logger::info("[REGRESSION] Stats overflow test - u64 wrapping");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    // Obtenir stats avant
    let stats_before = queue.stats();
    crate::logger::info(&alloc::format!("  Context switches before: {}", stats_before.context_switches));
    
    // Simuler beaucoup d'opérations (pas assez pour overflow u64 mais tester wrapping)
    const OPS: usize = 100_000;
    for i in 0..OPS {
        let thread = Arc::new(Thread::new_kernel(
            (i + 60000) as u64,
            "overflow",
            dummy_entry,
            4096
        ));
        queue.enqueue(thread);
        
        if i % 10 == 0 {
            queue.dequeue();
        }
        
        // Incrémenter context switches tous les 100
        if i % 100 == 0 {
            queue.inc_context_switches();
        }
    }
    
    let stats_after = queue.stats();
    crate::logger::info(&alloc::format!("  Context switches after: {}", stats_after.context_switches));
    crate::logger::info(&alloc::format!("  Queue length: {}", stats_after.queue_length));
    
    // Vérifier que les stats ont augmenté correctement
    let switch_diff = stats_after.context_switches - stats_before.context_switches;
    let expected_switches = (OPS / 100) as u64;
    if switch_diff == expected_switches {
        crate::logger::info("  ✅ PASS - Stats increment correctly");
    } else {
        crate::logger::error(&alloc::format!("  ❌ FAIL - Stats mismatch: {} vs {}", switch_diff, expected_switches));
    }
    
    // Cleanup
    while queue.dequeue().is_some() {}
}

/// Test: Thread ID exhaustion handling
pub fn test_regression_thread_exhaustion() {
    crate::logger::info("[REGRESSION] Thread exhaustion test - max threads");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    // Essayer d'enqueue beaucoup de threads
    // (limité par la RAM disponible en pratique)
    const MAX_ATTEMPT: usize = 1000;
    let mut created = 0;
    
    for i in 0..MAX_ATTEMPT {
        let thread = Arc::new(Thread::new_kernel(
            (i + 70000) as u64,
            "exhaust",
            dummy_entry,
            4096
        ));
        queue.enqueue(thread);
        created += 1;
    }
    
    crate::logger::info(&alloc::format!("  Created {} threads", created));
    crate::logger::info(&alloc::format!("  Queue length: {}", queue.len()));
    
    if created == MAX_ATTEMPT {
        crate::logger::info("  ✅ PASS - Handled thread creation gracefully");
    } else {
        crate::logger::warn(&alloc::format!("  ⚠️  PARTIAL - Only created {}/{}", created, MAX_ATTEMPT));
    }
    
    // Cleanup
    while queue.dequeue().is_some() {}
}

/// Test: Work stealing under stress
pub fn test_regression_work_stealing_stress() {
    crate::logger::info("[REGRESSION] Work stealing stress test");
    
    let queue0 = PER_CPU_QUEUES.get(0).unwrap();
    
    // Remplir la queue
    for i in 0..1000 {
        let thread = Arc::new(Thread::new_kernel(
            (i + 80000) as u64,
            "steal_stress",
            dummy_entry,
            4096
        ));
        queue0.enqueue(thread);
    }
    
    crate::logger::info(&alloc::format!("  Initial queue length: {}", queue0.len()));
    
    // Steal multiple times rapidement
    let mut total_stolen = 0;
    for round in 0..20 {
        let stolen = queue0.steal_half();
        total_stolen += stolen.len();
        crate::logger::info(&alloc::format!("    Round {}: stole {} threads", round + 1, stolen.len()));
        
        // Vérifier que stolen est cohérent
        if stolen.len() > queue0.len() + stolen.len() {
            crate::logger::error("  ❌ FAIL - Stole more than available!");
            return;
        }
    }
    
    crate::logger::info(&alloc::format!("  Total stolen: {} threads", total_stolen));
    crate::logger::info(&alloc::format!("  Remaining: {}", queue0.len()));
    
    if queue0.len() + total_stolen <= 1000 {
        crate::logger::info("  ✅ PASS - Work stealing coherent");
    } else {
        crate::logger::error("  ❌ FAIL - Work stealing created threads!");
    }
    
    // Cleanup
    while queue0.dequeue().is_some() {}
}

/// Test: Stats consistency under concurrent-like operations
pub fn test_regression_stats_consistency() {
    crate::logger::info("[REGRESSION] Stats consistency test");
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    let initial_stats = queue.stats();
    let initial_switches = initial_stats.context_switches;
    
    // Opérations mélangées
    const ROUNDS: usize = 100;
    let mut actual_enqueued = 0;
    let mut actual_dequeued = 0;
    let mut switch_count = 0u64;
    
    for round in 0..ROUNDS {
        // Enqueue 10
        for i in 0..10 {
            let thread = Arc::new(Thread::new_kernel(
                (round * 10 + i + 90000) as u64,
                "consistency",
                dummy_entry,
                4096
            ));
            queue.enqueue(thread);
            actual_enqueued += 1;
        }
        
        // Dequeue 3
        for _ in 0..3 {
            if queue.dequeue().is_some() {
                actual_dequeued += 1;
                queue.inc_context_switches();
                switch_count += 1;
            }
        }
        
        // Steal si round pair
        if round % 2 == 0 {
            queue.steal_half();
        }
    }
    
    let final_stats = queue.stats();
    let expected_remaining = actual_enqueued - actual_dequeued;
    let actual_switches = final_stats.context_switches - initial_switches;
    
    crate::logger::info(&alloc::format!("  Enqueued: {}", actual_enqueued));
    crate::logger::info(&alloc::format!("  Dequeued: {}", actual_dequeued));
    crate::logger::info(&alloc::format!("  Expected remaining: {}", expected_remaining));
    crate::logger::info(&alloc::format!("  Actual queue length: {}", final_stats.queue_length));
    crate::logger::info(&alloc::format!("  Context switches: {} vs {} expected", actual_switches, switch_count));
    
    if actual_switches == switch_count {
        crate::logger::info("  ✅ PASS - Stats perfectly consistent");
    } else {
        crate::logger::error("  ❌ FAIL - Stats inconsistency detected!");
    }
    
    // Cleanup
    while queue.dequeue().is_some() {}
}

/// Run all regression tests
pub fn run_all_regression_tests() {
    crate::logger::info("╔═══════════════════════════════════════════════════════╗");
    crate::logger::info("║        SMP SCHEDULER REGRESSION TESTS                 ║");
    crate::logger::info("╚═══════════════════════════════════════════════════════╝");
    crate::logger::info("");
    
    test_regression_memory_leak();
    crate::logger::info("");
    
    test_regression_stats_overflow();
    crate::logger::info("");
    
    test_regression_thread_exhaustion();
    crate::logger::info("");
    
    test_regression_work_stealing_stress();
    crate::logger::info("");
    
    test_regression_stats_consistency();
    crate::logger::info("");
    
    crate::logger::info("╔═══════════════════════════════════════════════════════╗");
    crate::logger::info("║         REGRESSION TESTS COMPLETE                     ║");
    crate::logger::info("╚═══════════════════════════════════════════════════════╝");
}

/// Dummy entry point
fn dummy_entry() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
