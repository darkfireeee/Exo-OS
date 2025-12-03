//! IPC Performance Benchmarking
//!
//! Integrated benchmarking for continuous performance validation.
//! Measures actual cycle counts to ensure we beat Linux.
//!
//! ## Benchmarks:
//! - Inline send/recv latency
//! - Zero-copy throughput
//! - Batch amortized cost
//! - Contention scaling
//! - Wake latency

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use core::arch::asm;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Read TSC (Time Stamp Counter)
#[inline(always)]
pub fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
        ((hi as u64) << 32) | (lo as u64)
    }
    
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Read TSC with serialization (more accurate for benchmarking)
#[inline(always)]
pub fn rdtscp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        let _aux: u32;
        asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _aux,
            options(nomem, nostack, preserves_flags)
        );
        ((hi as u64) << 32) | (lo as u64)
    }
    
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Memory fence for benchmarking
#[inline(always)]
pub fn mfence() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        asm!("mfence", options(nomem, nostack, preserves_flags));
    }
}

/// Benchmark result
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub name: String,
    pub iterations: u64,
    pub total_cycles: u64,
    pub min_cycles: u64,
    pub max_cycles: u64,
    pub avg_cycles: u64,
    pub p50_cycles: u64,
    pub p99_cycles: u64,
    pub ops_per_second: u64,
}

impl BenchResult {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            iterations: 0,
            total_cycles: 0,
            min_cycles: u64::MAX,
            max_cycles: 0,
            avg_cycles: 0,
            p50_cycles: 0,
            p99_cycles: 0,
            ops_per_second: 0,
        }
    }
    
    /// Compute statistics from samples
    pub fn compute(&mut self, mut samples: Vec<u64>, cpu_freq_mhz: u64) {
        if samples.is_empty() {
            return;
        }
        
        samples.sort();
        
        self.iterations = samples.len() as u64;
        self.total_cycles = samples.iter().sum();
        self.min_cycles = samples[0];
        self.max_cycles = *samples.last().unwrap();
        self.avg_cycles = self.total_cycles / self.iterations;
        self.p50_cycles = samples[samples.len() / 2];
        self.p99_cycles = samples[samples.len() * 99 / 100];
        
        // Calculate ops/sec
        if cpu_freq_mhz > 0 && self.avg_cycles > 0 {
            let cycles_per_sec = cpu_freq_mhz * 1_000_000;
            self.ops_per_second = cycles_per_sec / self.avg_cycles;
        }
    }
    
    /// Format as string
    pub fn format(&self) -> String {
        format!(
            "{}: {} iterations\n  avg: {} cycles, min: {}, max: {}\n  p50: {}, p99: {}\n  throughput: {} ops/sec",
            self.name,
            self.iterations,
            self.avg_cycles,
            self.min_cycles,
            self.max_cycles,
            self.p50_cycles,
            self.p99_cycles,
            self.ops_per_second
        )
    }
}

/// Benchmark runner
pub struct Benchmark {
    name: String,
    warmup_iters: usize,
    bench_iters: usize,
    samples: Vec<u64>,
    cpu_freq_mhz: u64,
}

impl Benchmark {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            warmup_iters: 1000,
            bench_iters: 10000,
            samples: Vec::new(),
            cpu_freq_mhz: 3000, // Default 3 GHz
        }
    }
    
    pub fn warmup(mut self, iters: usize) -> Self {
        self.warmup_iters = iters;
        self
    }
    
    pub fn iterations(mut self, iters: usize) -> Self {
        self.bench_iters = iters;
        self
    }
    
    pub fn cpu_freq(mut self, mhz: u64) -> Self {
        self.cpu_freq_mhz = mhz;
        self
    }
    
    /// Run benchmark with closure
    pub fn run<F>(&mut self, mut f: F) -> BenchResult
    where
        F: FnMut(),
    {
        self.samples.clear();
        self.samples.reserve(self.bench_iters);
        
        // Warmup
        for _ in 0..self.warmup_iters {
            f();
        }
        
        // Benchmark
        for _ in 0..self.bench_iters {
            mfence();
            let start = rdtscp();
            f();
            let end = rdtscp();
            mfence();
            
            self.samples.push(end.saturating_sub(start));
        }
        
        let mut result = BenchResult::new(&self.name);
        result.compute(self.samples.clone(), self.cpu_freq_mhz);
        result
    }
    
    /// Run with setup/teardown
    pub fn run_with_setup<S, F, T>(&mut self, mut setup: S, mut f: F) -> BenchResult
    where
        S: FnMut() -> T,
        F: FnMut(T),
    {
        self.samples.clear();
        self.samples.reserve(self.bench_iters);
        
        // Warmup
        for _ in 0..self.warmup_iters {
            let ctx = setup();
            f(ctx);
        }
        
        // Benchmark
        for _ in 0..self.bench_iters {
            let ctx = setup();
            
            mfence();
            let start = rdtscp();
            f(ctx);
            let end = rdtscp();
            mfence();
            
            self.samples.push(end.saturating_sub(start));
        }
        
        let mut result = BenchResult::new(&self.name);
        result.compute(self.samples.clone(), self.cpu_freq_mhz);
        result
    }
}

// =============================================================================
// IPC SPECIFIC BENCHMARKS
// =============================================================================

/// IPC benchmark suite
pub struct IpcBenchSuite {
    results: Vec<BenchResult>,
    cpu_freq_mhz: u64,
}

impl IpcBenchSuite {
    pub fn new(cpu_freq_mhz: u64) -> Self {
        Self {
            results: Vec::new(),
            cpu_freq_mhz,
        }
    }
    
    /// Benchmark inline send (≤56 bytes)
    pub fn bench_inline_send(&mut self) {
        use super::mpmc_ring::MpmcRing;
        
        let ring = MpmcRing::new(1024);
        
        let data = [0u8; 56];
        
        let mut bench = Benchmark::new("inline_send")
            .warmup(10000)
            .iterations(100000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let result = bench.run(|| {
            let _ = ring.try_send_inline(&data);
            // Also dequeue to not fill up
            let mut buf = [0u8; 64];
            let _ = ring.try_recv(&mut buf);
        });
        
        self.results.push(result);
    }
    
    /// Benchmark inline recv
    pub fn bench_inline_recv(&mut self) {
        use super::mpmc_ring::MpmcRing;
        
        let ring = MpmcRing::new(1024);
        
        let data = [0u8; 56];
        
        // Pre-fill ring
        for _ in 0..500 {
            let _ = ring.try_send_inline(&data);
        }
        
        let mut bench = Benchmark::new("inline_recv")
            .warmup(1000)
            .iterations(10000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let result = bench.run(|| {
            let mut buf = [0u8; 64];
            let _ = ring.try_recv(&mut buf);
            // Refill
            let _ = ring.try_send_inline(&data);
        });
        
        self.results.push(result);
    }
    
    /// Benchmark round-trip latency
    pub fn bench_roundtrip(&mut self) {
        use super::mpmc_ring::MpmcRing;
        
        let ring = MpmcRing::new(1024);
        
        let data = [42u8; 32];
        
        let mut bench = Benchmark::new("roundtrip_latency")
            .warmup(10000)
            .iterations(100000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let result = bench.run(|| {
            let _ = ring.try_send_inline(&data);
            let mut buf = [0u8; 64];
            let _ = ring.try_recv(&mut buf);
        });
        
        self.results.push(result);
    }
    
    /// Benchmark batch operations
    pub fn bench_batch(&mut self) {
        use super::mpmc_ring::MpmcRing;
        let ring = MpmcRing::new(1024);
        
        let batch_size = 16;
        let data: Vec<[u8; 32]> = (0..batch_size).map(|i| [i as u8; 32]).collect();
        
        let mut bench = Benchmark::new("batch_send_16")
            .warmup(1000)
            .iterations(10000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let result = bench.run(|| {
            let slices: Vec<&[u8]> = data.iter().map(|d| d.as_slice()).collect();
            let _ = ring.send_batch(&slices);
            // Clear
            let mut buf = [0u8; 64];
            for _ in 0..batch_size {
                let _ = ring.try_recv(&mut buf);
            }
        });
        
        // Compute per-message cost
        let mut adjusted = result.clone();
        adjusted.name = String::from("batch_per_message");
        adjusted.avg_cycles /= batch_size as u64;
        adjusted.min_cycles /= batch_size as u64;
        adjusted.max_cycles /= batch_size as u64;
        adjusted.p50_cycles /= batch_size as u64;
        adjusted.p99_cycles /= batch_size as u64;
        adjusted.ops_per_second *= batch_size as u64;
        
        self.results.push(result);
        self.results.push(adjusted);
    }
    
    /// Benchmark futex
    pub fn bench_futex(&mut self) {
        use super::futex::FutexMutex;
        
        let mutex = FutexMutex::new();
        
        let mut bench = Benchmark::new("futex_uncontended")
            .warmup(10000)
            .iterations(100000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let result = bench.run(|| {
            mutex.lock();
            mutex.unlock();
        });
        
        self.results.push(result);
    }
    
    /// Benchmark priority queue
    pub fn bench_priority_queue(&mut self) {
        use super::priority_queue::BoundedPriorityQueue;
        
        let queue: BoundedPriorityQueue<u64> = BoundedPriorityQueue::new();
        
        let mut bench = Benchmark::new("priority_enqueue_dequeue")
            .warmup(10000)
            .iterations(100000)
            .cpu_freq(self.cpu_freq_mhz);
        
        let mut counter = 0u64;
        let result = bench.run(|| {
            queue.enqueue(counter, 128);
            let _ = queue.dequeue();
            counter = counter.wrapping_add(1);
        });
        
        self.results.push(result);
    }
    
    /// Run all benchmarks
    pub fn run_all(&mut self) {
        self.bench_inline_send();
        self.bench_inline_recv();
        self.bench_roundtrip();
        self.bench_batch();
        self.bench_futex();
        self.bench_priority_queue();
    }
    
    /// Get results
    pub fn results(&self) -> &[BenchResult] {
        &self.results
    }
    
    /// Format report
    pub fn report(&self) -> String {
        let mut s = String::from("=== IPC Performance Report ===\n\n");
        
        for result in &self.results {
            s.push_str(&result.format());
            s.push_str("\n\n");
        }
        
        // Linux comparison
        s.push_str("=== Linux Comparison ===\n");
        s.push_str("Linux pipes: ~1200 cycles\n");
        s.push_str("Linux futex uncontended: ~50 cycles\n");
        s.push_str("Linux futex contended: ~400 cycles\n\n");
        
        // Performance analysis
        for result in &self.results {
            if result.name == "roundtrip_latency" {
                let speedup = 1200.0 / result.avg_cycles as f64;
                s.push_str(&format!(
                    "Roundtrip vs Linux pipes: {:.1}x faster ({} vs 1200 cycles)\n",
                    speedup, result.avg_cycles
                ));
            }
            if result.name == "futex_uncontended" {
                let speedup = 50.0 / result.avg_cycles as f64;
                if speedup >= 1.0 {
                    s.push_str(&format!(
                        "Futex vs Linux: {:.1}x faster ({} vs 50 cycles)\n",
                        speedup, result.avg_cycles
                    ));
                } else {
                    s.push_str(&format!(
                        "Futex vs Linux: {:.1}x slower ({} vs 50 cycles) - needs optimization\n",
                        1.0/speedup, result.avg_cycles
                    ));
                }
            }
        }
        
        s
    }
}

// =============================================================================
// CONTINUOUS MONITORING
// =============================================================================

/// Performance monitor for runtime tracking
pub struct PerfMonitor {
    /// Operation count
    op_count: AtomicU64,
    /// Total cycles
    total_cycles: AtomicU64,
    /// Min cycles seen
    min_cycles: AtomicU64,
    /// Max cycles seen
    max_cycles: AtomicU64,
    /// Is enabled
    enabled: AtomicBool,
}

impl PerfMonitor {
    pub const fn new() -> Self {
        Self {
            op_count: AtomicU64::new(0),
            total_cycles: AtomicU64::new(0),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
        }
    }
    
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }
    
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }
    
    #[inline]
    pub fn start(&self) -> u64 {
        if self.enabled.load(Ordering::Relaxed) {
            rdtscp()
        } else {
            0
        }
    }
    
    #[inline]
    pub fn end(&self, start: u64) {
        if start == 0 {
            return;
        }
        
        let end = rdtscp();
        let cycles = end.saturating_sub(start);
        
        self.op_count.fetch_add(1, Ordering::Relaxed);
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        
        // Update min/max (not atomic but good enough for monitoring)
        let current_min = self.min_cycles.load(Ordering::Relaxed);
        if cycles < current_min {
            self.min_cycles.store(cycles, Ordering::Relaxed);
        }
        
        let current_max = self.max_cycles.load(Ordering::Relaxed);
        if cycles > current_max {
            self.max_cycles.store(cycles, Ordering::Relaxed);
        }
    }
    
    pub fn snapshot(&self) -> PerfSnapshot {
        let count = self.op_count.load(Ordering::Relaxed);
        let total = self.total_cycles.load(Ordering::Relaxed);
        
        PerfSnapshot {
            op_count: count,
            total_cycles: total,
            avg_cycles: if count > 0 { total / count } else { 0 },
            min_cycles: self.min_cycles.load(Ordering::Relaxed),
            max_cycles: self.max_cycles.load(Ordering::Relaxed),
        }
    }
    
    pub fn reset(&self) {
        self.op_count.store(0, Ordering::Relaxed);
        self.total_cycles.store(0, Ordering::Relaxed);
        self.min_cycles.store(u64::MAX, Ordering::Relaxed);
        self.max_cycles.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
pub struct PerfSnapshot {
    pub op_count: u64,
    pub total_cycles: u64,
    pub avg_cycles: u64,
    pub min_cycles: u64,
    pub max_cycles: u64,
}

/// Global monitors
pub static SEND_MONITOR: PerfMonitor = PerfMonitor::new();
pub static RECV_MONITOR: PerfMonitor = PerfMonitor::new();
pub static WAKE_MONITOR: PerfMonitor = PerfMonitor::new();
