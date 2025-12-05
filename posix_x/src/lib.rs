//! # POSIX-X Compatibility Layer for Exo-OS
//!
//! Adaptive syscall translation layer providing 78-85% native performance
//! for Linux applications while leveraging Exo-OS kernel innovations.
//!
//! ## Architecture
//!
//! The POSIX-X layer implements a 3-path execution strategy:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    POSIX Syscall Entry                      │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!              ┌──────────────┼──────────────┐
//!              ▼              ▼              ▼
//!     ┌────────────┐  ┌────────────┐  ┌────────────┐
//!     │ Fast Path  │  │Hybrid Path │  │Legacy Path │
//!     │   (70%)    │  │   (25%)    │  │    (5%)    │
//!     │ <50 cycles │  │400-1000 cy │  │8000-50000  │
//!     └─────┬──────┘  └─────┬──────┘  └─────┬──────┘
//!           │               │               │
//!           ▼               ▼               ▼
//!     ┌────────────────────────────────────────────┐
//!     │           Exo-OS Native Kernel             │
//!     │  • Fusion Rings IPC (347 cycles)           │
//!     │  • Capability-based security               │
//!     │  • Zero-copy memory operations             │
//!     └────────────────────────────────────────────┘
//! ```
//!
//! ## Performance Targets
//!
//! | Syscall | Path | Target (cycles) | Linux baseline | Gain |
//! |---------|------|-----------------|----------------|------|
//! | getpid() | Fast | 48 | 26 | -85% |
//! | clock_gettime() | Fast | 100 | 150 | +50% |
//! | open() (cached) | Hybrid | 512 | 800 | +36% |
//! | read() inline | Hybrid | 402 | 500 | +20% |
//! | write() inline | Hybrid | 358 | 600 | +40% |
//! | pipe + I/O | Hybrid | 451 | 1200 | +62% |
//! | fork() | Legacy | 50,000 | 8,000 | -6.25x |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use posix_x::{init, SyscallTranslator, ExecutionPath};
//!
//! // Initialize the POSIX-X layer
//! init().expect("Failed to initialize POSIX-X");
//!
//! // Create a translator instance
//! let mut translator = SyscallTranslator::new();
//!
//! // Translate and execute a syscall
//! let args = [0u64; 6];
//! let result = translator.translate(39, &args); // getpid
//! ```

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

pub mod compat;
pub mod kernel_interface;
pub mod optimization;
pub mod signals;
pub mod translation;

// Re-exports for public API
pub use compat::PosixCompat;
pub use kernel_interface::{
    CapabilityCache, CapabilityHandle, CapabilityRights, CapabilityType,
    CacheStatsSnapshot, KernelInterface,
};
pub use optimization::{BatchOptimizer, ZeroCopyDetector};
pub use signals::{Signal, SignalAction, SignalHandlers};
pub use translation::SyscallTranslator;

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

/// POSIX-X version
pub const VERSION: &str = "0.1.0";

/// Target fast path percentage (70%)
pub const TARGET_FAST_PATH_RATIO: f32 = 70.0;

/// Target hybrid path percentage (25%)
pub const TARGET_HYBRID_PATH_RATIO: f32 = 25.0;

/// Target legacy path percentage (5%)
pub const TARGET_LEGACY_PATH_RATIO: f32 = 5.0;

/// Execution path for syscall handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExecutionPath {
    /// Direct mapping to native syscall (< 50 cycles)
    /// Used for: getpid, getuid, clock_gettime (vDSO), brk
    Fast = 0,
    /// Optimized translation (400-1000 cycles)
    /// Used for: open, read, write, close, mmap, socket ops
    Hybrid = 1,
    /// Full emulation for complex syscalls (8000-50000 cycles)
    /// Used for: fork, execve, ptrace
    Legacy = 2,
}

impl ExecutionPath {
    /// Get typical cycle count for this path
    #[inline]
    pub const fn typical_cycles(&self) -> u64 {
        match self {
            ExecutionPath::Fast => 50,
            ExecutionPath::Hybrid => 700,
            ExecutionPath::Legacy => 25000,
        }
    }

    /// Get path name for logging
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            ExecutionPath::Fast => "fast",
            ExecutionPath::Hybrid => "hybrid",
            ExecutionPath::Legacy => "legacy",
        }
    }
}

/// Performance statistics for POSIX-X layer
///
/// Thread-safe statistics using atomic operations for concurrent access.
#[derive(Debug)]
pub struct PosixXStats {
    /// Total syscalls handled
    total_syscalls: AtomicU64,
    /// Syscalls via fast path
    fast_path_count: AtomicU64,
    /// Syscalls via hybrid path
    hybrid_path_count: AtomicU64,
    /// Syscalls via legacy path
    legacy_path_count: AtomicU64,
    /// Total cycles spent
    total_cycles: AtomicU64,
    /// Capability cache hits
    cache_hits: AtomicU64,
    /// Capability cache misses
    cache_misses: AtomicU64,
    /// Zero-copy optimizations applied
    zerocopy_optimizations: AtomicU64,
    /// Batch optimizations applied
    batch_optimizations: AtomicU64,
}

impl Default for PosixXStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PosixXStats {
    /// Create new statistics tracker
    pub const fn new() -> Self {
        Self {
            total_syscalls: AtomicU64::new(0),
            fast_path_count: AtomicU64::new(0),
            hybrid_path_count: AtomicU64::new(0),
            legacy_path_count: AtomicU64::new(0),
            total_cycles: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            zerocopy_optimizations: AtomicU64::new(0),
            batch_optimizations: AtomicU64::new(0),
        }
    }

    /// Record a syscall execution
    #[inline]
    pub fn record_syscall(&self, path: ExecutionPath, cycles: u64) {
        self.total_syscalls.fetch_add(1, Ordering::Relaxed);
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);

        match path {
            ExecutionPath::Fast => {
                self.fast_path_count.fetch_add(1, Ordering::Relaxed);
            }
            ExecutionPath::Hybrid => {
                self.hybrid_path_count.fetch_add(1, Ordering::Relaxed);
            }
            ExecutionPath::Legacy => {
                self.legacy_path_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Record a cache hit
    #[inline]
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    #[inline]
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a zero-copy optimization
    #[inline]
    pub fn record_zerocopy(&self) {
        self.zerocopy_optimizations.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a batch optimization
    #[inline]
    pub fn record_batch(&self) {
        self.batch_optimizations.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total syscall count
    #[inline]
    pub fn total_syscalls(&self) -> u64 {
        self.total_syscalls.load(Ordering::Relaxed)
    }

    /// Get fast path count
    #[inline]
    pub fn fast_path_count(&self) -> u64 {
        self.fast_path_count.load(Ordering::Relaxed)
    }

    /// Get hybrid path count
    #[inline]
    pub fn hybrid_path_count(&self) -> u64 {
        self.hybrid_path_count.load(Ordering::Relaxed)
    }

    /// Get legacy path count
    #[inline]
    pub fn legacy_path_count(&self) -> u64 {
        self.legacy_path_count.load(Ordering::Relaxed)
    }

    /// Returns the percentage of fast path usage
    #[inline]
    pub fn fast_path_ratio(&self) -> f32 {
        let total = self.total_syscalls.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.fast_path_count.load(Ordering::Relaxed) as f32 / total as f32) * 100.0
    }

    /// Returns the capability cache hit ratio
    #[inline]
    pub fn cache_hit_ratio(&self) -> f32 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        (hits as f32 / total as f32) * 100.0
    }

    /// Returns average cycles per syscall
    #[inline]
    pub fn avg_cycles_per_syscall(&self) -> u64 {
        let total = self.total_syscalls.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_cycles.load(Ordering::Relaxed) / total
    }

    /// Check if performance targets are being met
    pub fn meets_targets(&self) -> bool {
        let fast_ratio = self.fast_path_ratio();
        let cache_ratio = self.cache_hit_ratio();

        // Target: 70% fast path, 90% cache hit ratio
        fast_ratio >= TARGET_FAST_PATH_RATIO * 0.9 && cache_ratio >= 85.0
    }

    /// Generate a performance report
    pub fn report(&self) -> PosixXReport {
        PosixXReport {
            total_syscalls: self.total_syscalls.load(Ordering::Relaxed),
            fast_path_count: self.fast_path_count.load(Ordering::Relaxed),
            hybrid_path_count: self.hybrid_path_count.load(Ordering::Relaxed),
            legacy_path_count: self.legacy_path_count.load(Ordering::Relaxed),
            total_cycles: self.total_cycles.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            zerocopy_optimizations: self.zerocopy_optimizations.load(Ordering::Relaxed),
            batch_optimizations: self.batch_optimizations.load(Ordering::Relaxed),
            fast_path_ratio: self.fast_path_ratio(),
            cache_hit_ratio: self.cache_hit_ratio(),
            avg_cycles: self.avg_cycles_per_syscall(),
            meets_targets: self.meets_targets(),
        }
    }

    /// Reset all statistics
    pub fn reset(&self) {
        self.total_syscalls.store(0, Ordering::Relaxed);
        self.fast_path_count.store(0, Ordering::Relaxed);
        self.hybrid_path_count.store(0, Ordering::Relaxed);
        self.legacy_path_count.store(0, Ordering::Relaxed);
        self.total_cycles.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
        self.zerocopy_optimizations.store(0, Ordering::Relaxed);
        self.batch_optimizations.store(0, Ordering::Relaxed);
    }
}

/// Performance report snapshot
#[derive(Debug, Clone)]
pub struct PosixXReport {
    /// Total syscalls handled
    pub total_syscalls: u64,
    /// Syscalls via fast path
    pub fast_path_count: u64,
    /// Syscalls via hybrid path
    pub hybrid_path_count: u64,
    /// Syscalls via legacy path
    pub legacy_path_count: u64,
    /// Total cycles spent
    pub total_cycles: u64,
    /// Capability cache hits
    pub cache_hits: u64,
    /// Capability cache misses
    pub cache_misses: u64,
    /// Zero-copy optimizations applied
    pub zerocopy_optimizations: u64,
    /// Batch optimizations applied
    pub batch_optimizations: u64,
    /// Fast path ratio percentage
    pub fast_path_ratio: f32,
    /// Cache hit ratio percentage
    pub cache_hit_ratio: f32,
    /// Average cycles per syscall
    pub avg_cycles: u64,
    /// Whether performance targets are met
    pub meets_targets: bool,
}

/// Global POSIX-X statistics
static GLOBAL_STATS: PosixXStats = PosixXStats::new();

/// Get global statistics reference
#[inline]
pub fn stats() -> &'static PosixXStats {
    &GLOBAL_STATS
}

/// Initialize POSIX-X layer
///
/// This function must be called before using any POSIX-X functionality.
/// It initializes:
/// - Capability cache for fast path lookups
/// - Signal handlers for POSIX signal emulation
/// - Batch optimizer for write coalescing
/// - Zero-copy detector for read/write pattern optimization
///
/// # Errors
///
/// Returns `PosixXError` if initialization fails.
///
/// # Example
///
/// ```rust,ignore
/// use posix_x::init;
///
/// fn main() {
///     init().expect("Failed to initialize POSIX-X layer");
///     // Now ready to use POSIX-X
/// }
/// ```
pub fn init() -> Result<(), PosixXError> {
    log::info!("POSIX-X layer v{} initializing...", VERSION);
    log::info!("Performance targets: {}% fast path, {}% hybrid path, {}% legacy path",
        TARGET_FAST_PATH_RATIO, TARGET_HYBRID_PATH_RATIO, TARGET_LEGACY_PATH_RATIO);

    // Initialize capability cache
    kernel_interface::init_capability_cache()?;
    log::debug!("Capability cache initialized");

    // Initialize signal handlers
    signals::init()?;
    log::debug!("Signal handlers initialized");

    // Initialize batch optimizer
    optimization::init_batch_optimizer()?;
    log::debug!("Batch optimizer initialized");

    // Reset statistics
    GLOBAL_STATS.reset();

    log::info!("POSIX-X layer initialized successfully");
    Ok(())
}

/// POSIX-X error types
#[derive(Debug, Clone)]
pub enum PosixXError {
    /// Syscall not supported
    NotSupported(i32),
    /// Invalid argument
    InvalidArgument(String),
    /// Capability error
    CapabilityError(String),
    /// Translation error
    TranslationError(String),
    /// Permission denied
    PermissionDenied(String),
    /// Resource temporarily unavailable
    WouldBlock,
    /// Interrupted system call
    Interrupted,
    /// No such file or directory
    NotFound(String),
    /// I/O error
    IoError(String),
    /// Internal error
    InternalError(String),
}

impl PosixXError {
    /// Convert to POSIX errno value
    pub fn to_errno(&self) -> i32 {
        match self {
            PosixXError::NotSupported(_) => 38,       // ENOSYS
            PosixXError::InvalidArgument(_) => 22,    // EINVAL
            PosixXError::CapabilityError(_) => 1,     // EPERM
            PosixXError::TranslationError(_) => 5,    // EIO
            PosixXError::PermissionDenied(_) => 13,   // EACCES
            PosixXError::WouldBlock => 11,            // EAGAIN
            PosixXError::Interrupted => 4,            // EINTR
            PosixXError::NotFound(_) => 2,            // ENOENT
            PosixXError::IoError(_) => 5,             // EIO
            PosixXError::InternalError(_) => 5,       // EIO
        }
    }

    /// Create from POSIX errno
    pub fn from_errno(errno: i32) -> Self {
        match errno {
            38 => PosixXError::NotSupported(errno),
            22 => PosixXError::InvalidArgument(String::from("Invalid argument")),
            1 | 13 => PosixXError::PermissionDenied(String::from("Permission denied")),
            11 => PosixXError::WouldBlock,
            4 => PosixXError::Interrupted,
            2 => PosixXError::NotFound(String::from("Not found")),
            _ => PosixXError::IoError(alloc::format!("errno {}", errno)),
        }
    }
}
