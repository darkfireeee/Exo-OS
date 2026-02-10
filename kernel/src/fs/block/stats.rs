//! I/O Statistics
//!
//! Tracks read/write throughput, latency histograms, and performance metrics.

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use alloc::string::String;
use alloc::vec::Vec;

/// I/O statistics for a block device
pub struct IoStats {
    /// Total bytes read
    bytes_read: AtomicU64,
    /// Total bytes written
    bytes_written: AtomicU64,
    /// Total read operations
    reads: AtomicU64,
    /// Total write operations
    writes: AtomicU64,
    /// Total flush operations
    flushes: AtomicU64,
    /// Total discard operations
    discards: AtomicU64,
    /// Read errors
    read_errors: AtomicU64,
    /// Write errors
    write_errors: AtomicU64,
    /// Total read latency (nanoseconds)
    total_read_latency_ns: AtomicU64,
    /// Total write latency (nanoseconds)
    total_write_latency_ns: AtomicU64,
    /// Max read latency
    max_read_latency_ns: AtomicU64,
    /// Max write latency
    max_write_latency_ns: AtomicU64,
    /// Sequential read count
    sequential_reads: AtomicU64,
    /// Random read count
    random_reads: AtomicU64,
    /// Sequential write count
    sequential_writes: AtomicU64,
    /// Random write count
    random_writes: AtomicU64,
    /// Last LBA accessed
    last_lba: AtomicU64,
    /// Timestamp of first operation
    start_time_ns: AtomicU64,
}

impl IoStats {
    /// Create new I/O statistics
    pub const fn new() -> Self {
        Self {
            bytes_read: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            flushes: AtomicU64::new(0),
            discards: AtomicU64::new(0),
            read_errors: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            total_read_latency_ns: AtomicU64::new(0),
            total_write_latency_ns: AtomicU64::new(0),
            max_read_latency_ns: AtomicU64::new(0),
            max_write_latency_ns: AtomicU64::new(0),
            sequential_reads: AtomicU64::new(0),
            random_reads: AtomicU64::new(0),
            sequential_writes: AtomicU64::new(0),
            random_writes: AtomicU64::new(0),
            last_lba: AtomicU64::new(0),
            start_time_ns: AtomicU64::new(0),
        }
    }

    /// Record a read operation
    pub fn record_read(&self, bytes: u64, latency_ns: u64, lba: u64, is_sequential: bool) {
        let now = crate::time::uptime_ns();

        if self.start_time_ns.load(Ordering::Relaxed) == 0 {
            self.start_time_ns.store(now, Ordering::Relaxed);
        }

        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.total_read_latency_ns.fetch_add(latency_ns, Ordering::Relaxed);

        let mut current_max = self.max_read_latency_ns.load(Ordering::Relaxed);
        while latency_ns > current_max {
            match self.max_read_latency_ns.compare_exchange_weak(
                current_max,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }

        if is_sequential {
            self.sequential_reads.fetch_add(1, Ordering::Relaxed);
        } else {
            self.random_reads.fetch_add(1, Ordering::Relaxed);
        }

        self.last_lba.store(lba, Ordering::Relaxed);
    }

    /// Record a write operation
    pub fn record_write(&self, bytes: u64, latency_ns: u64, lba: u64, is_sequential: bool) {
        let now = crate::time::uptime_ns();

        if self.start_time_ns.load(Ordering::Relaxed) == 0 {
            self.start_time_ns.store(now, Ordering::Relaxed);
        }

        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.total_write_latency_ns.fetch_add(latency_ns, Ordering::Relaxed);

        let mut current_max = self.max_write_latency_ns.load(Ordering::Relaxed);
        while latency_ns > current_max {
            match self.max_write_latency_ns.compare_exchange_weak(
                current_max,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }

        if is_sequential {
            self.sequential_writes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.random_writes.fetch_add(1, Ordering::Relaxed);
        }

        self.last_lba.store(lba, Ordering::Relaxed);
    }

    /// Record a flush operation
    pub fn record_flush(&self) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a discard operation
    pub fn record_discard(&self, _bytes: u64) {
        self.discards.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a read error
    pub fn record_read_error(&self) {
        self.read_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a write error
    pub fn record_write_error(&self) {
        self.write_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total bytes read
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Get total bytes written
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Get total read operations
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// Get total write operations
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    /// Get average read latency in nanoseconds
    pub fn avg_read_latency_ns(&self) -> u64 {
        let reads = self.reads.load(Ordering::Relaxed);
        if reads == 0 {
            return 0;
        }
        self.total_read_latency_ns.load(Ordering::Relaxed) / reads
    }

    /// Get average write latency in nanoseconds
    pub fn avg_write_latency_ns(&self) -> u64 {
        let writes = self.writes.load(Ordering::Relaxed);
        if writes == 0 {
            return 0;
        }
        self.total_write_latency_ns.load(Ordering::Relaxed) / writes
    }

    /// Get max read latency in nanoseconds
    pub fn max_read_latency_ns(&self) -> u64 {
        self.max_read_latency_ns.load(Ordering::Relaxed)
    }

    /// Get max write latency in nanoseconds
    pub fn max_write_latency_ns(&self) -> u64 {
        self.max_write_latency_ns.load(Ordering::Relaxed)
    }

    /// Get read throughput in bytes per second
    pub fn read_throughput_bps(&self) -> u64 {
        let elapsed = self.elapsed_time_ns();
        if elapsed == 0 {
            return 0;
        }
        let bytes = self.bytes_read.load(Ordering::Relaxed);
        (bytes * 1_000_000_000) / elapsed
    }

    /// Get write throughput in bytes per second
    pub fn write_throughput_bps(&self) -> u64 {
        let elapsed = self.elapsed_time_ns();
        if elapsed == 0 {
            return 0;
        }
        let bytes = self.bytes_written.load(Ordering::Relaxed);
        (bytes * 1_000_000_000) / elapsed
    }

    /// Get read IOPS
    pub fn read_iops(&self) -> u64 {
        let elapsed = self.elapsed_time_ns();
        if elapsed == 0 {
            return 0;
        }
        let reads = self.reads.load(Ordering::Relaxed);
        (reads * 1_000_000_000) / elapsed
    }

    /// Get write IOPS
    pub fn write_iops(&self) -> u64 {
        let elapsed = self.elapsed_time_ns();
        if elapsed == 0 {
            return 0;
        }
        let writes = self.writes.load(Ordering::Relaxed);
        (writes * 1_000_000_000) / elapsed
    }

    /// Get sequential read percentage
    pub fn sequential_read_pct(&self) -> u64 {
        let total = self.reads.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let sequential = self.sequential_reads.load(Ordering::Relaxed);
        (sequential * 100) / total
    }

    /// Get sequential write percentage
    pub fn sequential_write_pct(&self) -> u64 {
        let total = self.writes.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let sequential = self.sequential_writes.load(Ordering::Relaxed);
        (sequential * 100) / total
    }

    /// Get error rate (errors per 1000 operations)
    pub fn error_rate(&self) -> u64 {
        let total = self.reads.load(Ordering::Relaxed) + self.writes.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let errors = self.read_errors.load(Ordering::Relaxed) +
                     self.write_errors.load(Ordering::Relaxed);
        (errors * 1000) / total
    }

    /// Get elapsed time since first operation
    fn elapsed_time_ns(&self) -> u64 {
        let start = self.start_time_ns.load(Ordering::Relaxed);
        if start == 0 {
            return 0;
        }
        let now = crate::time::uptime_ns();
        now.saturating_sub(start)
    }

    /// Reset all statistics
    pub fn reset(&self) {
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.reads.store(0, Ordering::Relaxed);
        self.writes.store(0, Ordering::Relaxed);
        self.flushes.store(0, Ordering::Relaxed);
        self.discards.store(0, Ordering::Relaxed);
        self.read_errors.store(0, Ordering::Relaxed);
        self.write_errors.store(0, Ordering::Relaxed);
        self.total_read_latency_ns.store(0, Ordering::Relaxed);
        self.total_write_latency_ns.store(0, Ordering::Relaxed);
        self.max_read_latency_ns.store(0, Ordering::Relaxed);
        self.max_write_latency_ns.store(0, Ordering::Relaxed);
        self.sequential_reads.store(0, Ordering::Relaxed);
        self.random_reads.store(0, Ordering::Relaxed);
        self.sequential_writes.store(0, Ordering::Relaxed);
        self.random_writes.store(0, Ordering::Relaxed);
        self.last_lba.store(0, Ordering::Relaxed);
        self.start_time_ns.store(crate::time::uptime_ns(), Ordering::Relaxed);
    }

    /// Get snapshot of current statistics
    pub fn snapshot(&self) -> IoStatsSnapshot {
        IoStatsSnapshot {
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            flushes: self.flushes.load(Ordering::Relaxed),
            discards: self.discards.load(Ordering::Relaxed),
            read_errors: self.read_errors.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            avg_read_latency_ns: self.avg_read_latency_ns(),
            avg_write_latency_ns: self.avg_write_latency_ns(),
            max_read_latency_ns: self.max_read_latency_ns.load(Ordering::Relaxed),
            max_write_latency_ns: self.max_write_latency_ns.load(Ordering::Relaxed),
            read_throughput_bps: self.read_throughput_bps(),
            write_throughput_bps: self.write_throughput_bps(),
            read_iops: self.read_iops(),
            write_iops: self.write_iops(),
            sequential_read_pct: self.sequential_read_pct(),
            sequential_write_pct: self.sequential_write_pct(),
            error_rate: self.error_rate(),
        }
    }
}

impl Default for IoStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of I/O statistics (non-atomic copy)
#[derive(Debug, Clone, Copy)]
pub struct IoStatsSnapshot {
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub reads: u64,
    pub writes: u64,
    pub flushes: u64,
    pub discards: u64,
    pub read_errors: u64,
    pub write_errors: u64,
    pub avg_read_latency_ns: u64,
    pub avg_write_latency_ns: u64,
    pub max_read_latency_ns: u64,
    pub max_write_latency_ns: u64,
    pub read_throughput_bps: u64,
    pub write_throughput_bps: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    pub sequential_read_pct: u64,
    pub sequential_write_pct: u64,
    pub error_rate: u64,
}

/// Latency histogram for detailed latency analysis
pub struct LatencyHistogram {
    /// Buckets: < 100us, < 500us, < 1ms, < 5ms, < 10ms, < 50ms, < 100ms, >= 100ms
    buckets: [AtomicU32; 8],
}

impl LatencyHistogram {
    pub const fn new() -> Self {
        const INIT: AtomicU32 = AtomicU32::new(0);
        Self {
            buckets: [INIT; 8],
        }
    }

    /// Record a latency measurement
    pub fn record(&self, latency_ns: u64) {
        let latency_us = latency_ns / 1000;
        let bucket = match latency_us {
            0..=99 => 0,
            100..=499 => 1,
            500..=999 => 2,
            1000..=4999 => 3,
            5000..=9999 => 4,
            10000..=49999 => 5,
            50000..=99999 => 6,
            _ => 7,
        };
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Get bucket counts
    pub fn buckets(&self) -> [u32; 8] {
        [
            self.buckets[0].load(Ordering::Relaxed),
            self.buckets[1].load(Ordering::Relaxed),
            self.buckets[2].load(Ordering::Relaxed),
            self.buckets[3].load(Ordering::Relaxed),
            self.buckets[4].load(Ordering::Relaxed),
            self.buckets[5].load(Ordering::Relaxed),
            self.buckets[6].load(Ordering::Relaxed),
            self.buckets[7].load(Ordering::Relaxed),
        ]
    }

    /// Get bucket labels
    pub fn bucket_labels() -> [&'static str; 8] {
        [
            "< 100us",
            "< 500us",
            "< 1ms",
            "< 5ms",
            "< 10ms",
            "< 50ms",
            "< 100ms",
            ">= 100ms",
        ]
    }

    /// Reset histogram
    pub fn reset(&self) {
        for bucket in &self.buckets {
            bucket.store(0, Ordering::Relaxed);
        }
    }

    /// Get total count
    pub fn total(&self) -> u32 {
        self.buckets.iter()
            .map(|b| b.load(Ordering::Relaxed))
            .sum()
    }

    /// Get percentile (approximate)
    pub fn percentile(&self, p: u8) -> u64 {
        let total = self.total();
        if total == 0 {
            return 0;
        }

        let target = (total as u64 * p as u64) / 100;
        let mut count = 0u64;

        for (i, bucket) in self.buckets.iter().enumerate() {
            count += bucket.load(Ordering::Relaxed) as u64;
            if count >= target {
                return match i {
                    0 => 50_000,
                    1 => 300_000,
                    2 => 750_000,
                    3 => 3_000_000,
                    4 => 7_500_000,
                    5 => 30_000_000,
                    6 => 75_000_000,
                    _ => 150_000_000,
                };
            }
        }

        150_000_000
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}
