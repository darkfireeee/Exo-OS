//! Context switch benchmarking

use core::arch::x86_64::_rdtsc;

#[inline]
fn read_tsc_wrapper() -> u64 {
    unsafe { _rdtsc() }
}
use super::SwitchStats;

/// Benchmark structure
pub struct SwitchBenchmark {
    stats: SwitchStats,
}

impl SwitchBenchmark {
    pub fn new() -> Self {
        Self {
            stats: SwitchStats::default(),
        }
    }
    
    /// Benchmark a context switch
    pub fn benchmark_once<F>(&mut self, f: F) -> u64
    where
        F: FnOnce(),
    {
        let start = read_tsc_wrapper();
        f();
        let end = read_tsc_wrapper();
        let cycles = end - start;
        
        self.stats.record_switch(cycles);
        cycles
    }
    
    /// Get statistics
    pub fn stats(&self) -> &SwitchStats {
        &self.stats
    }
    
    /// Reset statistics
    pub fn reset(&mut self) {
        self.stats = SwitchStats::default();
    }
}

impl Default for SwitchBenchmark {
    fn default() -> Self {
        Self::new()
    }
}

/// Benchmark context switch performance
pub fn benchmark_switch<F>(iterations: usize, f: F) -> SwitchStats
where
    F: Fn(),
{
    let mut benchmark = SwitchBenchmark::new();
    
    for _ in 0..iterations {
        benchmark.benchmark_once(|| f());
    }
    
    *benchmark.stats()
}
