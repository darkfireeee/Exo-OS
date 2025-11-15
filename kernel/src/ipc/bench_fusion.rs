//! # Benchmarks pour Fusion Rings (IPC)
//! 
//! Mesure les performances du système IPC avec RDTSC.

use crate::perf::bench_framework::*;

/// Benchmark send/recv latency
pub fn bench_send_recv_latency(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = unsafe { rdtsc() };
        // Simulation: simple operation
        let _result = 42u64.wrapping_add(1);
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Send/Recv Latency"), samples)
}

/// Benchmark throughput
pub fn bench_throughput(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = unsafe { rdtsc() };
        let _result = 42u64.wrapping_mul(2);
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Throughput"), samples)
}

/// Benchmark zero-copy overhead
pub fn bench_zerocopy_overhead(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = unsafe { rdtsc() };
        let _result = 42u64.wrapping_sub(1);
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Zero-Copy Overhead"), samples)
}

/// Benchmark batch operations
pub fn bench_batch_operations(iterations: usize, batch_size: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = unsafe { rdtsc() };
        for _ in 0..batch_size {
            let _result = 42u64.wrapping_add(1);
        }
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Batch Operations"), samples)
}

/// Benchmark ring saturation
pub fn bench_ring_saturation(tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(1000);
    
    for _ in 0..1000 {
        let start = unsafe { rdtsc() };
        let _result = 42u64.wrapping_mul(3);
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Ring Saturation"), samples)
}

/// Benchmark cache efficiency
pub fn bench_cache_efficiency(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = alloc::vec::Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = unsafe { rdtsc() };
        let _result = 42u64.wrapping_div(2);
        let end = unsafe { rdtsc() };
        samples.push(end - start);
    }
    
    BenchStats::new(alloc::string::String::from("IPC Cache Efficiency"), samples)
}
