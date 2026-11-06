// ipc/rpc/timeout.rs — RPC timeout management for Exo-OS
//
// This module implements the timeout and retry policy for RPC calls:
//   - `RpcTimeout`: timeout configuration + exponential backoff
//   - `RpcDeadline`: absolute deadline computed from boot timestamp
//   - `RetryPolicy`: retry policy (N attempts + backoff)
//   - Constants: RPC_DEFAULT_TIMEOUT_NS, RPC_MAX_RETRIES, etc.
//
// RULE TIMEOUT-01: no hardware timer here — the module reads the TSC counter
//                  via crate::arch::tsc::rdtsc() (or an fn pointer).
// RULE TIMEOUT-02: exponential backoff bounded by RPC_MAX_BACKOFF_NS.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// Timeout constants
// ---------------------------------------------------------------------------

/// Default RPC timeout: 5 ms in nanoseconds
pub const RPC_DEFAULT_TIMEOUT_NS: u64 = 5_000_000;

/// Minimum RPC timeout: 100 µs
pub const RPC_MIN_TIMEOUT_NS: u64 = 100_000;

/// Maximum RPC timeout: 1 s
pub const RPC_MAX_TIMEOUT_NS: u64 = 1_000_000_000;

/// Maximum number of attempts per RPC call (includes first attempt)
pub const RPC_MAX_RETRIES: u32 = 3;

/// Initial backoff delay: 1 ms
pub const RPC_INITIAL_BACKOFF_NS: u64 = 1_000_000;

/// Maximum backoff delay: 50 ms
pub const RPC_MAX_BACKOFF_NS: u64 = 50_000_000;

/// Backoff multiplication factor (fixedpoint x2)
pub const RPC_BACKOFF_FACTOR: u64 = 2;

// ---------------------------------------------------------------------------
// Time source
// ---------------------------------------------------------------------------

/// Current time reading function type (in nanoseconds since boot)
pub type TimeFn = fn() -> u64;

/// Default time function: returns 0 (usable before clock init)
#[allow(dead_code)]
fn time_zero() -> u64 {
    0
}

/// Configurable time function pointer
/// Initialized to 0 (casting fn pointer forbidden in const, installed at runtime).
static TIME_FN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Installs a time reading function.
/// Must be called by the arch layer during TSC init.
pub fn install_time_fn(f: TimeFn) {
    TIME_FN.store(f as usize, Ordering::Release);
}

/// Reads current time via installed function.
/// Returns 0 if clock is not yet initialized (before install_time_fn()).
pub fn now_ns() -> u64 {
    let f_ptr = TIME_FN.load(Ordering::Relaxed);
    if f_ptr == 0 {
        // Clock not initialized: return 0 (before TSC init).
        return 0;
    }
    // SAFETY: f_ptr was stored via install_time_fn() from a valid TimeFn.
    let f: TimeFn = unsafe { core::mem::transmute(f_ptr) };
    f()
}

// ---------------------------------------------------------------------------
// RpcTimeout — timeout configuration
// ---------------------------------------------------------------------------

/// Timeout configuration for an RPC call.
#[derive(Debug, Clone, Copy)]
pub struct RpcTimeout {
    /// Timeout duration in ns (0 = use RPC_DEFAULT_TIMEOUT_NS)
    pub duration_ns: u64,
    /// Start timestamp (ns since boot), 0 = uninitialized
    pub start_ns: u64,
}

impl RpcTimeout {
    /// Timeout with specific duration
    pub fn with_ns(duration_ns: u64) -> Self {
        let d = duration_ns.clamp(RPC_MIN_TIMEOUT_NS, RPC_MAX_TIMEOUT_NS);
        Self {
            duration_ns: d,
            start_ns: now_ns(),
        }
    }

    /// Default timeout (5 ms)
    pub fn default() -> Self {
        Self::with_ns(RPC_DEFAULT_TIMEOUT_NS)
    }

    /// Creates infinite timeout (indefinite wait)
    pub fn infinite() -> Self {
        Self {
            duration_ns: 0,
            start_ns: 0,
        }
    }

    /// Checks if timeout is infinite
    pub fn is_infinite(&self) -> bool {
        self.duration_ns == 0
    }

    /// Starts timeout (records start timestamp)
    pub fn start(&mut self) {
        self.start_ns = now_ns();
    }

    /// Checks if timeout has expired
    pub fn is_expired(&self) -> bool {
        if self.is_infinite() {
            return false;
        }
        if self.start_ns == 0 {
            return false;
        }
        let elapsed = now_ns().saturating_sub(self.start_ns);
        elapsed >= self.duration_ns
    }

    /// Remaining time in ns (0 if expired or infinite)
    pub fn remaining_ns(&self) -> u64 {
        if self.is_infinite() {
            return u64::MAX;
        }
        if self.start_ns == 0 {
            return self.duration_ns;
        }
        let elapsed = now_ns().saturating_sub(self.start_ns);
        self.duration_ns.saturating_sub(elapsed)
    }

    /// Suggested number of spins to await this timeout.
    /// Heuristic: 1 ns ≈ 1 spin @ ~1 GHz.
    pub fn to_spin_max(&self) -> u64 {
        if self.is_infinite() {
            return 0;
        }
        self.remaining_ns()
    }
}

// ---------------------------------------------------------------------------
// RpcDeadline — absolute deadline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct RpcDeadline {
    pub deadline_ns: u64,
}

impl RpcDeadline {
    pub fn from_timeout(t: &RpcTimeout) -> Self {
        if t.is_infinite() {
            return Self {
                deadline_ns: u64::MAX,
            };
        }
        let start = if t.start_ns == 0 {
            now_ns()
        } else {
            t.start_ns
        };
        Self {
            deadline_ns: start.saturating_add(t.duration_ns),
        }
    }

    pub fn is_expired(&self) -> bool {
        now_ns() >= self.deadline_ns
    }

    pub fn remaining_ns(&self) -> u64 {
        self.deadline_ns.saturating_sub(now_ns())
    }
}

// ---------------------------------------------------------------------------
// RetryPolicy — retry policy with exponential backoff
// ---------------------------------------------------------------------------

/// Internal state of a retryer.
pub struct RetryState {
    pub max_retries: u32,
    pub attempt: u32,
    pub backoff_ns: u64,
    pub timeout: RpcTimeout,
}

impl RetryState {
    /// Initializes a retry policy with timeout.
    pub fn new(max_retries: u32, timeout: RpcTimeout) -> Self {
        let n = if max_retries > RPC_MAX_RETRIES {
            RPC_MAX_RETRIES
        } else {
            max_retries
        };
        Self {
            max_retries: n,
            attempt: 0,
            backoff_ns: RPC_INITIAL_BACKOFF_NS,
            timeout,
        }
    }

    /// Default policy (max retries = RPC_MAX_RETRIES, timeout = default)
    pub fn default() -> Self {
        Self::new(RPC_MAX_RETRIES, RpcTimeout::default())
    }

    /// Checks if a new attempt is possible.
    pub fn should_retry(&self) -> bool {
        self.attempt < self.max_retries && !self.timeout.is_expired()
    }

    /// Prepares next attempt.
    /// Returns `Ok(wait_ns)` if retry is possible, `Err(Timeout)` otherwise.
    pub fn next_attempt(&mut self) -> Result<u64, IpcError> {
        if self.attempt >= self.max_retries {
            return Err(IpcError::Retry);
        }
        if self.timeout.is_expired() {
            return Err(IpcError::Timeout);
        }
        self.attempt += 1;
        let wait = self.backoff_ns;
        // Bounded exponential backoff
        self.backoff_ns = self
            .backoff_ns
            .saturating_mul(RPC_BACKOFF_FACTOR)
            .min(RPC_MAX_BACKOFF_NS);
        Ok(wait)
    }

    /// Spin-waits for `ns` nanoseconds (iteration approximation).
    pub fn spin_wait_ns(ns: u64) {
        let spins = ns.min(10_000_000); // bounded to 10M iterations max
        for _ in 0..spins {
            core::hint::spin_loop();
        }
    }

    /// Number of performed attempts
    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

// ---------------------------------------------------------------------------
// Global RPC timeout statistics
// ---------------------------------------------------------------------------

pub struct RpcTimeoutStats {
    pub total_timeouts: AtomicU64,
    pub total_retries: AtomicU64,
    pub total_success_after_retry: AtomicU64,
}

impl RpcTimeoutStats {
    pub const fn new() -> Self {
        Self {
            total_timeouts: AtomicU64::new(0),
            total_retries: AtomicU64::new(0),
            total_success_after_retry: AtomicU64::new(0),
        }
    }

    pub fn record_timeout(&self) {
        self.total_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.total_retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success_after_retry(&self) {
        self.total_success_after_retry
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RpcTimeoutStatsSnapshot {
        RpcTimeoutStatsSnapshot {
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
            total_retries: self.total_retries.load(Ordering::Relaxed),
            total_success_after_retry: self.total_success_after_retry.load(Ordering::Relaxed),
        }
    }
}

pub static RPC_TIMEOUT_STATS: RpcTimeoutStats = RpcTimeoutStats::new();

#[derive(Debug, Clone, Copy)]
pub struct RpcTimeoutStatsSnapshot {
    pub total_timeouts: u64,
    pub total_retries: u64,
    pub total_success_after_retry: u64,
}
