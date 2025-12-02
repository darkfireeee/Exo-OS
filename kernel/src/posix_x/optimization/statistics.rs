//! Statistics Collection for POSIX-X Optimization
//!
//! Collects and aggregates performance statistics

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

/// Per-syscall statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct SyscallStats {
    /// Total number of calls
    pub call_count: u64,
    /// Total execution time (nanoseconds)
    pub total_time_ns: u64,
    /// Minimum execution time
    pub min_time_ns: u64,
    /// Maximum execution time
    pub max_time_ns: u64,
    /// Number of errors
    pub error_count: u64,
    /// Bytes transferred (for I/O syscalls)
    pub bytes_transferred: u64,
}

impl SyscallStats {
    fn new() -> Self {
        Self {
            call_count: 0,
            total_time_ns: 0,
            min_time_ns: u64::MAX,
            max_time_ns: 0,
            error_count: 0,
            bytes_transferred: 0,
        }
    }

    fn record(&mut self, duration_ns: u64, success: bool, bytes: u64) {
        self.call_count += 1;
        self.total_time_ns += duration_ns;
        self.min_time_ns = self.min_time_ns.min(duration_ns);
        self.max_time_ns = self.max_time_ns.max(duration_ns);

        if !success {
            self.error_count += 1;
        }

        self.bytes_transferred += bytes;
    }

    pub fn avg_time_ns(&self) -> u64 {
        if self.call_count > 0 {
            self.total_time_ns / self.call_count
        } else {
            0
        }
    }

    pub fn error_rate(&self) -> f64 {
        if self.call_count > 0 {
            (self.error_count as f64 / self.call_count as f64) * 100.0
        } else {
            0.0
        }
    }

    pub fn throughput_mbps(&self) -> f64 {
        if self.total_time_ns > 0 {
            let seconds = self.total_time_ns as f64 / 1_000_000_000.0;
            let megabytes = self.bytes_transferred as f64 / 1_048_576.0;
            megabytes / seconds
        } else {
            0.0
        }
    }
}

/// Global statistics collector
pub struct StatisticsCollector {
    /// Per-syscall statistics
    syscall_stats: RwLock<BTreeMap<usize, SyscallStats>>,
    /// Global counters
    total_syscalls: AtomicU64,
    total_errors: AtomicU64,
    total_time_ns: AtomicU64,
    /// Collection enabled
    enabled: AtomicU64,
}

impl StatisticsCollector {
    /// Create new statistics collector
    pub const fn new() -> Self {
        Self {
            syscall_stats: RwLock::new(BTreeMap::new()),
            total_syscalls: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_time_ns: AtomicU64::new(0),
            enabled: AtomicU64::new(1),
        }
    }

    /// Record a syscall execution
    pub fn record(
        &self,
        syscall_num: usize,
        duration_ns: u64,
        success: bool,
        bytes_transferred: u64,
    ) {
        if self.enabled.load(Ordering::Relaxed) == 0 {
            return;
        }

        // Update global counters
        self.total_syscalls.fetch_add(1, Ordering::Relaxed);
        self.total_time_ns.fetch_add(duration_ns, Ordering::Relaxed);

        if !success {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
        }

        // Update per-syscall stats
        let mut stats = self.syscall_stats.write();
        let entry = stats.entry(syscall_num).or_insert_with(SyscallStats::new);
        entry.record(duration_ns, success, bytes_transferred);
    }

    /// Get statistics for a specific syscall
    pub fn get_syscall_stats(&self, syscall_num: usize) -> Option<SyscallStats> {
        self.syscall_stats.read().get(&syscall_num).copied()
    }

    /// Get all syscall statistics
    pub fn get_all_stats(&self) -> alloc::vec::Vec<(usize, SyscallStats)> {
        self.syscall_stats
            .read()
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect()
    }

    /// Get global statistics
    pub fn get_global_stats(&self) -> GlobalStats {
        GlobalStats {
            total_syscalls: self.total_syscalls.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            total_time_ns: self.total_time_ns.load(Ordering::Relaxed),
            avg_time_ns: {
                let total = self.total_syscalls.load(Ordering::Relaxed);
                let time = self.total_time_ns.load(Ordering::Relaxed);
                if total > 0 {
                    time / total
                } else {
                    0
                }
            },
            error_rate: {
                let total = self.total_syscalls.load(Ordering::Relaxed) as f64;
                let errors = self.total_errors.load(Ordering::Relaxed) as f64;
                if total > 0.0 {
                    (errors / total) * 100.0
                } else {
                    0.0
                }
            },
        }
    }

    /// Get top N most called syscalls
    pub fn get_top_syscalls(&self, n: usize) -> alloc::vec::Vec<(usize, SyscallStats)> {
        let mut stats: alloc::vec::Vec<_> = self
            .syscall_stats
            .read()
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();

        stats.sort_by(|a, b| b.1.call_count.cmp(&a.1.call_count));
        stats.truncate(n);
        stats
    }

    /// Get slowest syscalls
    pub fn get_slowest_syscalls(&self, n: usize) -> alloc::vec::Vec<(usize, SyscallStats)> {
        let mut stats: alloc::vec::Vec<_> = self
            .syscall_stats
            .read()
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();

        stats.sort_by(|a, b| b.1.avg_time_ns().cmp(&a.1.avg_time_ns()));
        stats.truncate(n);
        stats
    }

    /// Enable/disable statistics collection
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled as u64, Ordering::Relaxed);
    }

    /// Reset all statistics
    pub fn reset(&self) {
        self.syscall_stats.write().clear();
        self.total_syscalls.store(0, Ordering::Relaxed);
        self.total_errors.store(0, Ordering::Relaxed);
        self.total_time_ns.store(0, Ordering::Relaxed);
    }

    /// Export statistics as formatted string
    pub fn export_report(&self) -> alloc::string::String {
        use alloc::format;
        use alloc::string::String;

        let global = self.get_global_stats();
        let mut report = String::new();

        report.push_str("=== POSIX-X Statistics Report ===\n\n");
        report.push_str(&format!("Total Syscalls: {}\n", global.total_syscalls));
        report.push_str(&format!(
            "Total Errors: {} ({:.2}%)\n",
            global.total_errors, global.error_rate
        ));
        report.push_str(&format!("Average Time: {} ns\n\n", global.avg_time_ns));

        report.push_str("Top 10 Most Called Syscalls:\n");
        for (num, stats) in self.get_top_syscalls(10) {
            report.push_str(&format!(
                "  Syscall {}: {} calls, avg {} ns\n",
                num,
                stats.call_count,
                stats.avg_time_ns()
            ));
        }

        report.push_str("\nTop 10 Slowest Syscalls:\n");
        for (num, stats) in self.get_slowest_syscalls(10) {
            report.push_str(&format!(
                "  Syscall {}: avg {} ns, max {} ns\n",
                num,
                stats.avg_time_ns(),
                stats.max_time_ns
            ));
        }

        report
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GlobalStats {
    pub total_syscalls: u64,
    pub total_errors: u64,
    pub total_time_ns: u64,
    pub avg_time_ns: u64,
    pub error_rate: f64,
}

/// Global statistics collector instance
pub static STATISTICS_COLLECTOR: StatisticsCollector = StatisticsCollector::new();

/// Record a syscall (convenience function)
#[inline]
pub fn record_syscall(syscall_num: usize, duration_ns: u64, success: bool, bytes_transferred: u64) {
    STATISTICS_COLLECTOR.record(syscall_num, duration_ns, success, bytes_transferred);
}

/// Get global statistics
pub fn get_global_stats() -> GlobalStats {
    STATISTICS_COLLECTOR.get_global_stats()
}
