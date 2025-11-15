//! # Module de Performance et Benchmarking
//! 
//! Framework unifié pour mesurer et comparer les performances des optimisations.

pub mod bench_framework;

#[cfg(test)]
pub mod bench_orchestrator;

// Re-exports
pub use bench_framework::{
    rdtsc,
    calibrate_tsc_frequency,
    BenchStats,
    BenchComparison,
    BenchmarkSuite,
    run_benchmark_with_retry,
};
