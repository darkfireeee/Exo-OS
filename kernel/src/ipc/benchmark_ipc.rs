//! IPC Performance Benchmarks
//!
//! Measures latency and throughput of IPC operations

use super::fusion_ring::{FusionRing, inline, sync::RingSync};
use super::{Message, MessageHeader, MessageType};
use core::sync::atomic::{AtomicU64, Ordering};

/// Benchmark results
#[derive(Debug, Clone, Copy)]
pub struct BenchmarkResults {
    /// Operation name
    pub name: &'static str,
    
    /// Number of iterations
    pub iterations: u64,
    
    /// Total cycles
    pub total_cycles: u64,
    
    /// Minimum cycles
    pub min_cycles: u64,
    
    /// Maximum cycles
    pub max_cycles: u64,
    
    /// Average cycles
    pub avg_cycles: u64,
    
    /// Median cycles
    pub median_cycles: u64,
}

impl BenchmarkResults {
    pub fn new(name: &'static str, iterations: u64) -> Self {
        Self {
            name,
            iterations,
            total_cycles: 0,
            min_cycles: u64::MAX,
            max_cycles: 0,
            avg_cycles: 0,
            median_cycles: 0,
        }
    }
    
    pub fn record(&mut self, cycles: u64) {
        self.total_cycles += cycles;
        if cycles < self.min_cycles {
            self.min_cycles = cycles;
        }
        if cycles > self.max_cycles {
            self.max_cycles = cycles;
        }
    }
    
    pub fn finalize(&mut self) {
        if self.iterations > 0 {
            self.avg_cycles = self.total_cycles / self.iterations;
        }
    }
    
    pub fn print(&self) {
        log::info!("=== {} ===", self.name);
        log::info!("  Iterations: {}", self.iterations);
        log::info!("  Total:      {} cycles", self.total_cycles);
        log::info!("  Average:    {} cycles", self.avg_cycles);
        log::info!("  Min:        {} cycles", self.min_cycles);
        log::info!("  Max:        {} cycles", self.max_cycles);
        log::info!("  Median:     {} cycles", self.median_cycles);
    }
}

/// Read CPU timestamp counter
#[inline(always)]
fn rdtsc() -> u64 {
    unsafe {
        let mut low: u32;
        let mut high: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

/// Benchmark inline message send
pub fn bench_inline_send(iterations: u64) -> BenchmarkResults {
    let mut results = BenchmarkResults::new("Inline Send (≤56B)", iterations);
    
    // Create ring
    let ring = super::fusion_ring::ring::Ring::new(64);
    let data = b"Hello, World! This is a test message for benchmarking."; // 54 bytes
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = inline::send_inline(&ring, data);
        let end = rdtsc();
        
        results.record(end - start);
        
        // Clear ring for next iteration
        let _ = inline::recv_inline(&ring, &mut [0u8; 64]);
    }
    
    results.finalize();
    results
}

/// Benchmark inline message receive
pub fn bench_inline_recv(iterations: u64) -> BenchmarkResults {
    let mut results = BenchmarkResults::new("Inline Recv (≤56B)", iterations);
    
    let ring = super::fusion_ring::ring::Ring::new(64);
    let data = b"Hello, World! This is a test message for benchmarking.";
    let mut buffer = [0u8; 64];
    
    for _ in 0..iterations {
        // Pre-fill ring
        let _ = inline::send_inline(&ring, data);
        
        let start = rdtsc();
        let _ = inline::recv_inline(&ring, &mut buffer);
        let end = rdtsc();
        
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark round-trip (send + receive)
pub fn bench_roundtrip(iterations: u64) -> BenchmarkResults {
    let mut results = BenchmarkResults::new("Round-trip (send + recv)", iterations);
    
    let ring = super::fusion_ring::ring::Ring::new(64);
    let data = b"Test message";
    let mut buffer = [0u8; 64];
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = inline::send_inline(&ring, data);
        let _ = inline::recv_inline(&ring, &mut buffer);
        let end = rdtsc();
        
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark blocking send (with sync)
pub fn bench_blocking_send(iterations: u64) -> BenchmarkResults {
    let mut results = BenchmarkResults::new("Blocking Send", iterations);
    
    let ring = super::fusion_ring::ring::Ring::new(64);
    let sync = RingSync::new();
    let data = b"Test message";
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = sync::send_blocking(&ring, &sync, data);
        let end = rdtsc();
        
        results.record(end - start);
        
        // Clear ring
        let _ = sync::recv_blocking(&ring, &sync, &mut [0u8; 64]);
    }
    
    results.finalize();
    results
}

/// Benchmark message creation (inline)
pub fn bench_message_creation(iterations: u64) -> BenchmarkResults {
    let mut results = BenchmarkResults::new("Message Creation (inline)", iterations);
    
    let header = MessageHeader::new(MessageType::Data, 1, 2);
    let data = b"Test message for benchmarking";
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = Message::new_inline(header, data);
        let end = rdtsc();
        
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Run all IPC benchmarks
pub fn run_all_benchmarks(iterations: u64) {
    log::info!("=== IPC Performance Benchmarks ===");
    log::info!("Running {} iterations per test\n", iterations);
    
    let inline_send = bench_inline_send(iterations);
    inline_send.print();
    
    let inline_recv = bench_inline_recv(iterations);
    inline_recv.print();
    
    let roundtrip = bench_roundtrip(iterations);
    roundtrip.print();
    
    let blocking = bench_blocking_send(iterations);
    blocking.print();
    
    let msg_create = bench_message_creation(iterations);
    msg_create.print();
    
    log::info!("\n=== Summary ===");
    log::info!("Target: <350 cycles for inline operations");
    
    if inline_send.avg_cycles < 350 {
        log::info!("✓ Inline send: {} cycles (PASS)", inline_send.avg_cycles);
    } else {
        log::warn!("✗ Inline send: {} cycles (FAIL, target <350)", inline_send.avg_cycles);
    }
    
    if inline_recv.avg_cycles < 350 {
        log::info!("✓ Inline recv: {} cycles (PASS)", inline_recv.avg_cycles);
    } else {
        log::warn!("✗ Inline recv: {} cycles (FAIL, target <350)", inline_recv.avg_cycles);
    }
    
    if roundtrip.avg_cycles < 700 {
        log::info!("✓ Round-trip: {} cycles (PASS)", roundtrip.avg_cycles);
    } else {
        log::warn!("✗ Round-trip: {} cycles (FAIL, target <700)", roundtrip.avg_cycles);
    }
}

/// Quick benchmark (10 iterations)
pub fn quick_bench() {
    run_all_benchmarks(10);
}

/// Standard benchmark (1000 iterations)
pub fn standard_bench() {
    run_all_benchmarks(1000);
}

/// Extensive benchmark (100000 iterations)
pub fn extensive_bench() {
    run_all_benchmarks(100000);
}
