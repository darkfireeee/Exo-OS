//! # syscall/dispatch/stats.rs — Syscall dispatch instrumentation and counters
//!
//! Measures and records syscall pipeline statistics:
//! - Total requests processed
//! - Breakdown fast-path vs slow-path table
//! - ENOSYS errors
//! - Applied Linux compat numbers
//! - TSC latency sampling

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::cpu::tsc::read_tsc;

// ─────────────────────────────────────────────────────────────────────────────
// Atomic instrumentation counters
// ─────────────────────────────────────────────────────────────────────────────

static DISPATCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static DISPATCH_FAST_PATH: AtomicU64 = AtomicU64::new(0);
static DISPATCH_SLOW_PATH: AtomicU64 = AtomicU64::new(0);
static DISPATCH_ENOSYS: AtomicU64 = AtomicU64::new(0);
static DISPATCH_COMPAT: AtomicU64 = AtomicU64::new(0);
/// Sum of dispatch latencies (TSC cycles). Sampled 1/256.
static DISPATCH_LATENCY_CYC: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn inc_total() {
    DISPATCH_TOTAL.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn inc_fast_path() {
    DISPATCH_FAST_PATH.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn inc_slow_path() {
    DISPATCH_SLOW_PATH.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn inc_enosys() {
    DISPATCH_ENOSYS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn inc_compat() {
    DISPATCH_COMPAT.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn sample_latency(tsc_start: u64) {
    let tsc_end = read_tsc();
    let elapsed = tsc_end.saturating_sub(tsc_start);
    // Sampling: bit 7 of tsc_end to avoid expensive modulo (1/256).
    if (tsc_end & 0xFF) == 0 {
        DISPATCH_LATENCY_CYC.fetch_add(elapsed, Ordering::Relaxed);
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions))]
#[inline]
pub fn syscall_trace(_message: &[u8]) {}

#[cfg(not(all(target_arch = "x86_64", debug_assertions)))]
#[inline]
pub fn syscall_trace(_message: &[u8]) {}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
#[inline]
pub fn exec_trace(message: &[u8]) {
    crate::arch::x86_64::terminal::debug_write(message);
}

#[cfg(not(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace)))]
#[inline]
pub fn exec_trace(_message: &[u8]) {}

/// Snapshot of dispatch counters.
#[derive(Copy, Clone, Debug, Default)]
pub struct DispatchStats {
    pub total: u64,
    pub fast_path: u64,
    pub slow_path: u64,
    pub enosys: u64,
    pub compat: u64,
    pub latency_cyc: u64,
}

/// Returns an instant snapshot of counters.
pub fn dispatch_stats() -> DispatchStats {
    DispatchStats {
        total: DISPATCH_TOTAL.load(Ordering::Relaxed),
        fast_path: DISPATCH_FAST_PATH.load(Ordering::Relaxed),
        slow_path: DISPATCH_SLOW_PATH.load(Ordering::Relaxed),
        enosys: DISPATCH_ENOSYS.load(Ordering::Relaxed),
        compat: DISPATCH_COMPAT.load(Ordering::Relaxed),
        latency_cyc: DISPATCH_LATENCY_CYC.load(Ordering::Relaxed),
    }
}

/// Resets all dispatch counters to zero.
/// Used by tests and profiling startup.
pub fn reset_dispatch_stats() {
    DISPATCH_TOTAL.store(0, Ordering::Relaxed);
    DISPATCH_FAST_PATH.store(0, Ordering::Relaxed);
    DISPATCH_SLOW_PATH.store(0, Ordering::Relaxed);
    DISPATCH_ENOSYS.store(0, Ordering::Relaxed);
    DISPATCH_COMPAT.store(0, Ordering::Relaxed);
    DISPATCH_LATENCY_CYC.store(0, Ordering::Relaxed);
}
