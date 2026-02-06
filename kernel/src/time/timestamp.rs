//! Timestamp utilities for IPC and kernel timing
//!
//! Provides monotonic cycle counter for low-overhead timing

use super::tsc;

/// Get monotonic cycle count (uses TSC)
#[inline]
pub fn monotonic_cycles() -> u64 {
    tsc::read_tsc()
}

/// Convert cycles to nanoseconds
#[inline]
pub fn cycles_to_ns(cycles: u64) -> u64 {
    tsc::cycles_to_ns(cycles)
}

/// Convert nanoseconds to cycles
#[inline]
pub fn ns_to_cycles(ns: u64) -> u64 {
    tsc::ns_to_cycles(ns)
}
