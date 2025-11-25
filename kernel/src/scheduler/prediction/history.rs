//! Execution history tracking

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Maximum history size
const MAX_HISTORY: usize = 32;

/// Execution history
pub struct ExecutionHistory {
    /// Recent runtimes (nanoseconds)
    runtimes: VecDeque<u64>,
    /// Total runtime
    total_ns: u64,
    /// Total executions
    count: u64,
}

impl ExecutionHistory {
    pub fn new() -> Self {
        Self {
            runtimes: VecDeque::with_capacity(MAX_HISTORY),
            total_ns: 0,
            count: 0,
        }
    }
    
    /// Add runtime sample
    pub fn add_sample(&mut self, runtime_ns: u64) {
        if self.runtimes.len() >= MAX_HISTORY {
            self.runtimes.pop_front();
        }
        self.runtimes.push_back(runtime_ns);
        self.total_ns += runtime_ns;
        self.count += 1;
    }
    
    /// Get average runtime
    pub fn average(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ns / self.count
        }
    }
    
    /// Get recent average (last N samples)
    pub fn recent_average(&self, n: usize) -> u64 {
        let samples: Vec<_> = self.runtimes.iter().rev().take(n).copied().collect();
        if samples.is_empty() {
            0
        } else {
            samples.iter().sum::<u64>() / samples.len() as u64
        }
    }
    
    /// Get variance
    pub fn variance(&self) -> u64 {
        if self.runtimes.is_empty() {
            return 0;
        }
        
        let avg = self.average();
        let sum_sq_diff: u64 = self.runtimes
            .iter()
            .map(|&x| {
                let diff = if x > avg { x - avg } else { avg - x };
                diff * diff
            })
            .sum();
        
        sum_sq_diff / self.runtimes.len() as u64
    }
}

impl Default for ExecutionHistory {
    fn default() -> Self {
        Self::new()
    }
}
