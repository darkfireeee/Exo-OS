//! SMP Scheduler Performance Benchmarks

use crate::scheduler::core::percpu_queue::PER_CPU_QUEUES;
use crate::scheduler::smp_init::current_cpu_id;
use crate::scheduler::thread::Thread;
use crate::arch::x86_64::smp::SMP_SYSTEM;
use crate::time::tsc::read_tsc;
use alloc::sync::Arc;

/// Benchmark: Local enqueue latency
pub fn bench_local_enqueue() -> (u64, u64, u64) {
    const ITERATIONS: usize = 1000;
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    let mut total_cycles = 0u64;
    let mut min_cycles = u64::MAX;
    let mut max_cycles = 0u64;
    
    for i in 0..ITERATIONS {
        let thread = Arc::new(Thread::new_kernel(
            (5000 + i) as u64,
            "bench_thread",
            dummy_entry,
            4096
        ));
        
        let start = read_tsc();
        queue.enqueue(thread);
        let end = read_tsc();
        
        let cycles = end.saturating_sub(start);
        total_cycles += cycles;
        min_cycles = min_cycles.min(cycles);
        max_cycles = max_cycles.max(cycles);
    }
    
    // Cleanup
    while queue.dequeue().is_some() {}
    
    let avg = total_cycles / ITERATIONS as u64;
    (avg, min_cycles, max_cycles)
}

/// Benchmark: Local dequeue latency
pub fn bench_local_dequeue() -> (u64, u64, u64) {
    const ITERATIONS: usize = 1000;
    
    let cpu_id = current_cpu_id();
    let queue = PER_CPU_QUEUES.get(cpu_id).unwrap();
    
    // Pre-fill queue
    for i in 0..ITERATIONS {
        let thread = Arc::new(Thread::new_kernel(
            (6000 + i) as u64,
            "bench_thread",
            dummy_entry,
            4096
        ));
        queue.enqueue(thread);
    }
    
    let mut total_cycles = 0u64;
    let mut min_cycles = u64::MAX;
    let mut max_cycles = 0u64;
    
    for _ in 0..ITERATIONS {
        let start = read_tsc();
        let _ = queue.dequeue();
        let end = read_tsc();
        
        let cycles = end.saturating_sub(start);
        total_cycles += cycles;
        min_cycles = min_cycles.min(cycles);
        max_cycles = max_cycles.max(cycles);
    }
    
    let avg = total_cycles / ITERATIONS as u64;
    (avg, min_cycles, max_cycles)
}

/// Benchmark: Work stealing latency
pub fn bench_work_stealing() -> Option<(u64, u64, u64)> {
    if SMP_SYSTEM.cpu_count() < 2 {
        return None;
    }
    
    const ITERATIONS: usize = 100;
    const THREADS_PER_ITER: usize = 20;
    
    let queue0 = PER_CPU_QUEUES.get(0).unwrap();
    
    let mut total_cycles = 0u64;
    let mut min_cycles = u64::MAX;
    let mut max_cycles = 0u64;
    
    for iter in 0..ITERATIONS {
        // Fill queue
        for i in 0..THREADS_PER_ITER {
            let thread = Arc::new(Thread::new_kernel(
                (7000 + iter * THREADS_PER_ITER + i) as u64,
                "steal_bench",
                dummy_entry,
                4096
            ));
            queue0.enqueue(thread);
        }
        
        // Measure steal
        let start = read_tsc();
        let stolen = queue0.steal_half();
        let end = read_tsc();
        
        let cycles = end.saturating_sub(start);
        total_cycles += cycles;
        min_cycles = min_cycles.min(cycles);
        max_cycles = max_cycles.max(cycles);
        
        // Cleanup stolen
        drop(stolen);
    }
    
    // Final cleanup
    while queue0.dequeue().is_some() {}
    
    let avg = total_cycles / ITERATIONS as u64;
    Some((avg, min_cycles, max_cycles))
}

/// Benchmark: current_cpu_id() latency
pub fn bench_cpu_id() -> (u64, u64, u64) {
    const ITERATIONS: usize = 10000;
    
    let mut total_cycles = 0u64;
    let mut min_cycles = u64::MAX;
    let mut max_cycles = 0u64;
    
    for _ in 0..ITERATIONS {
        let start = read_tsc();
        let _ = current_cpu_id();
        let end = read_tsc();
        
        let cycles = end.saturating_sub(start);
        total_cycles += cycles;
        min_cycles = min_cycles.min(cycles);
        max_cycles = max_cycles.max(cycles);
    }
    
    let avg = total_cycles / ITERATIONS as u64;
    (avg, min_cycles, max_cycles)
}

/// Run all benchmarks and display results
pub fn run_all_benchmarks() {
    crate::logger::info("╔═══════════════════════════════════════════════════════╗");
    crate::logger::info("║        SMP SCHEDULER PERFORMANCE BENCHMARKS           ║");
    crate::logger::info("╚═══════════════════════════════════════════════════════╝");
    
    // CPU ID benchmark
    crate::logger::info("\n[BENCH] current_cpu_id() latency:");
    let (avg, min, max) = bench_cpu_id();
    crate::logger::info(&alloc::format!("  Average: {} cycles", avg));
    crate::logger::info(&alloc::format!("  Min:     {} cycles", min));
    crate::logger::info(&alloc::format!("  Max:     {} cycles", max));
    crate::logger::info(&alloc::format!("  Target:  <10 cycles - {}", 
        if avg < 10 { "✅ PASS" } else { "❌ FAIL" }));
    
    // Enqueue benchmark
    crate::logger::info("\n[BENCH] Local enqueue latency (1000 iterations):");
    let (avg, min, max) = bench_local_enqueue();
    crate::logger::info(&alloc::format!("  Average: {} cycles", avg));
    crate::logger::info(&alloc::format!("  Min:     {} cycles", min));
    crate::logger::info(&alloc::format!("  Max:     {} cycles", max));
    crate::logger::info(&alloc::format!("  Target:  <100 cycles - {}", 
        if avg < 100 { "✅ PASS" } else { "❌ FAIL" }));
    
    // Dequeue benchmark
    crate::logger::info("\n[BENCH] Local dequeue latency (1000 iterations):");
    let (avg, min, max) = bench_local_dequeue();
    crate::logger::info(&alloc::format!("  Average: {} cycles", avg));
    crate::logger::info(&alloc::format!("  Min:     {} cycles", min));
    crate::logger::info(&alloc::format!("  Max:     {} cycles", max));
    crate::logger::info(&alloc::format!("  Target:  <100 cycles - {}", 
        if avg < 100 { "✅ PASS" } else { "❌ FAIL" }));
    
    // Work stealing benchmark
    if let Some((avg, min, max)) = bench_work_stealing() {
        crate::logger::info("\n[BENCH] Work stealing latency (100 iterations, 20 threads each):");
        crate::logger::info(&alloc::format!("  Average: {} cycles", avg));
        crate::logger::info(&alloc::format!("  Min:     {} cycles", min));
        crate::logger::info(&alloc::format!("  Max:     {} cycles", max));
        crate::logger::info(&alloc::format!("  Target:  <5000 cycles - {}", 
            if avg < 5000 { "✅ PASS" } else { "❌ FAIL" }));
    } else {
        crate::logger::warn("\n[BENCH] Work stealing: Skipped (need ≥2 CPUs)");
    }
    
    // Summary
    crate::logger::info("\n╔═══════════════════════════════════════════════════════╗");
    crate::logger::info("║              BENCHMARK COMPLETE                        ║");
    crate::logger::info("╚═══════════════════════════════════════════════════════╝");
}

/// Dummy thread entry
fn dummy_entry() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
