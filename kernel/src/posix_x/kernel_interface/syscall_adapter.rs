//! Syscall Adapter - High-Level Syscall Orchestration
//!
//! Coordinates syscall execution with optimization, profiling, and error handling

use crate::posix_x::optimization::{ADAPTIVE_OPTIMIZER, STATISTICS_COLLECTOR};
use crate::posix_x::tools::profiler::PROFILER;

/// Syscall execution context
pub struct SyscallContext {
    pub syscall_num: usize,
    pub args: [u64; 6],
    pub caller_pid: u64,
}

impl SyscallContext {
    pub fn new(syscall_num: usize, args: [u64; 6], caller_pid: u64) -> Self {
        Self {
            syscall_num,
            args,
            caller_pid,
        }
    }
}

/// Execute syscall with full instrumentation
pub fn execute_syscall(ctx: &SyscallContext) -> Result<u64, i32> {
    let start_time = current_time_ns();

    // Record for adaptive optimizer
    let _strategy = ADAPTIVE_OPTIMIZER.get_strategy(ctx.syscall_num);

    // Execute syscall through dispatch
    let result_raw = crate::syscall::dispatch::dispatch_syscall(ctx.syscall_num as u64, &ctx.args);

    let end_time = current_time_ns();
    let duration = end_time - start_time;

    // Convert i64 result to Result<u64, i32>
    let result = if result_raw >= 0 {
        Ok(result_raw as u64)
    } else {
        Err((-result_raw) as i32)
    };

    // Record statistics
    let success = result.is_ok();
    let bytes_transferred = if is_io_syscall(ctx.syscall_num) {
        result.unwrap_or(0)
    } else {
        0
    };

    STATISTICS_COLLECTOR.record(ctx.syscall_num, duration, success, bytes_transferred);

    // Record for adaptive optimizer
    ADAPTIVE_OPTIMIZER.record_syscall(ctx.syscall_num, duration, &ctx.args);

    // Record for profiler (if enabled)
    PROFILER.record(ctx.syscall_num, ctx.args, duration, result_raw);

    result
}

/// Check if syscall is I/O related
fn is_io_syscall(syscall_num: usize) -> bool {
    matches!(
        syscall_num,
        0 | 1 |     // read, write
        19 | 20 |   // readv, writev
        40 |        // sendfile
        75 | 76 // splice, tee
    )
}

/// Get current time in nanoseconds
fn current_time_ns() -> u64 {
    // Would use TSC or similar
    // Placeholder for now
    0
}

/// Initialize syscall adapter
pub fn init() {
    log::debug!("Syscall adapter initialized");
}
