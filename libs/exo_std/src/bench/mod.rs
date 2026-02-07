//! Benchmarking utilities for exo_std
//!
//! Provides simple performance measurement tools for benchmarking
//! synchronization primitives, collections, and other components.

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use core::time::Duration;
use crate::time::Instant;

pub mod sync;
pub mod collections;

/// Result of a benchmark run
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Name of the benchmark
    pub name: String,
    /// Number of iterations
    pub iterations: u64,
    /// Total duration
    pub total_duration: Duration,
    /// Average duration per iteration
    pub avg_duration: Duration,
    /// Minimum duration observed
    pub min_duration: Duration,
    /// Maximum duration observed
    pub max_duration: Duration,
}

impl BenchmarkResult {
    /// Get throughput in operations per second
    pub fn ops_per_sec(&self) -> f64 {
        let secs = self.total_duration.as_secs_f64();
        if secs > 0.0 {
            self.iterations as f64 / secs
        } else {
            0.0
        }
    }

    /// Get average nanoseconds per operation
    pub fn avg_nanos(&self) -> u64 {
        self.avg_duration.as_nanos() as u64
    }
}

/// Benchmark runner
pub struct Benchmark {
    name: String,
    iterations: u64,
}

impl Benchmark {
    /// Create a new benchmark
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            iterations: 1000,
        }
    }

    /// Set the number of iterations
    pub fn iterations(mut self, n: u64) -> Self {
        self.iterations = n;
        self
    }

    /// Run the benchmark
    pub fn run<F>(self, mut f: F) -> BenchmarkResult
    where
        F: FnMut(),
    {
        let mut durations = Vec::with_capacity(self.iterations as usize);

        // Warmup
        for _ in 0..10 {
            f();
        }

        // Actual benchmark
        for _ in 0..self.iterations {
            let start = Instant::now();
            f();
            let elapsed = start.elapsed();
            durations.push(elapsed);
        }

        // Calculate statistics
        let total_duration: Duration = durations.iter().sum();
        let avg_duration = total_duration / self.iterations as u32;

        let mut min_duration = durations[0];
        let mut max_duration = durations[0];

        for &d in &durations {
            if d < min_duration {
                min_duration = d;
            }
            if d > max_duration {
                max_duration = d;
            }
        }

        BenchmarkResult {
            name: self.name,
            iterations: self.iterations,
            total_duration,
            avg_duration,
            min_duration,
            max_duration,
        }
    }
}

/// Compare two benchmark results
pub fn compare(baseline: &BenchmarkResult, comparison: &BenchmarkResult) -> f64 {
    let baseline_nanos = baseline.avg_nanos() as f64;
    let comparison_nanos = comparison.avg_nanos() as f64;

    if baseline_nanos > 0.0 {
        (comparison_nanos / baseline_nanos - 1.0) * 100.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_simple() {
        let result = Benchmark::new("test")
            .iterations(100)
            .run(|| {
                // Simple operation
                let _x = 1 + 1;
            });

        assert_eq!(result.iterations, 100);
        assert!(result.avg_duration.as_nanos() > 0);
    }

    #[test]
    fn test_benchmark_result_ops_per_sec() {
        let result = BenchmarkResult {
            name: String::from("test"),
            iterations: 1000,
            total_duration: Duration::from_secs(1),
            avg_duration: Duration::from_millis(1),
            min_duration: Duration::from_micros(500),
            max_duration: Duration::from_millis(2),
        };

        assert_eq!(result.ops_per_sec(), 1000.0);
    }

    #[test]
    fn test_compare() {
        let baseline = BenchmarkResult {
            name: String::from("baseline"),
            iterations: 100,
            total_duration: Duration::from_millis(100),
            avg_duration: Duration::from_millis(1),
            min_duration: Duration::from_micros(500),
            max_duration: Duration::from_millis(2),
        };

        let faster = BenchmarkResult {
            name: String::from("faster"),
            iterations: 100,
            total_duration: Duration::from_millis(50),
            avg_duration: Duration::from_micros(500),
            min_duration: Duration::from_micros(250),
            max_duration: Duration::from_millis(1),
        };

        let change = compare(&baseline, &faster);
        assert!(change < 0.0); // Faster is negative percentage change
    }
}
