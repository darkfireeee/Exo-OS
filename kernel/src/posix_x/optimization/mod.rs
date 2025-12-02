//! Optimization Module
//!
//! Provides adaptive optimization, zero-copy detection, batching, and statistics

pub mod adaptive;
pub mod batching;
pub mod cache;
pub mod statistics;
pub mod zerocopy;

pub use adaptive::{AdaptiveOptimizer, OptimizationStrategy, ADAPTIVE_OPTIMIZER};
pub use batching::{BatchOptimizer, BATCH_OPTIMIZER};
pub use statistics::{StatisticsCollector, SyscallStats, STATISTICS_COLLECTOR};
pub use zerocopy::{ZeroCopyDetector, ZeroCopyStrategy, ZEROCOPY_DETECTOR};

/// Initialize all optimization subsystems
pub fn init() {
    log::info!("Initializing POSIX-X optimization subsystems");

    // Optimizers are statically initialized
    // Just log their status
    log::debug!("Adaptive optimizer: enabled");
    log::debug!("Zero-copy detector: enabled");
    log::debug!("Batch optimizer: enabled");
    log::debug!("Statistics collector: enabled");
}

/// Get comprehensive optimization report
pub fn get_optimization_report() -> alloc::string::String {
    use alloc::format;
    use alloc::string::String;

    let mut report = String::new();

    report.push_str("=== POSIX-X Optimization Report ===\n\n");

    // Statistics
    report.push_str(&STATISTICS_COLLECTOR.export_report());
    report.push_str("\n");

    // Zero-copy stats
    let zc_stats = ZEROCOPY_DETECTOR.get_stats();
    report.push_str(&format!("Zero-Copy Statistics:\n"));
    report.push_str(&format!(
        "  Opportunities: {}\n",
        zc_stats.opportunities_found
    ));
    report.push_str(&format!("  Executions: {}\n", zc_stats.executions));
    report.push_str(&format!("  Hit Rate: {:.2}%\n", zc_stats.hit_rate));
    report.push_str(&format!("  Bytes Saved: {}\n\n", zc_stats.bytes_saved));

    // Batch stats
    let batch_stats = BATCH_OPTIMIZER.get_stats();
    report.push_str(&format!("Batching Statistics:\n"));
    report.push_str(&format!("  Calls Batched: {}\n", batch_stats.calls_batched));
    report.push_str(&format!(
        "  Batches Executed: {}\n",
        batch_stats.batches_executed
    ));
    report.push_str(&format!(
        "  Average Batch Size: {:.2}\n",
        batch_stats.avg_batch_size
    ));
    report.push_str(&format!("  Pending: {}\n", batch_stats.pending_batches));

    report
}

/// Enable all optimizations
pub fn enable_all() {
    ADAPTIVE_OPTIMIZER.set_enabled(true);
    BATCH_OPTIMIZER.set_enabled(true);
    STATISTICS_COLLECTOR.set_enabled(true);
    log::info!("All POSIX-X optimizations enabled");
}

/// Disable all optimizations  
pub fn disable_all() {
    ADAPTIVE_OPTIMIZER.set_enabled(false);
    BATCH_OPTIMIZER.set_enabled(false);
    STATISTICS_COLLECTOR.set_enabled(false);
    log::info!("All POSIX-X optimizations disabled");
}
