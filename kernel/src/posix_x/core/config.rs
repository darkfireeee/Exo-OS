//! POSIX-X Runtime Configuration
//!
//! Configurable parameters for POSIX compatibility layer

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Runtime configuration for POSIX-X
pub struct PosixConfig {
    /// Enable strict POSIX compliance (vs performance)
    strict_mode: AtomicBool,

    /// Maximum file descriptors per process
    max_fds: AtomicUsize,

    /// Enable optimization (batching, zero-copy, etc.)
    enable_optimization: AtomicBool,

    /// Enable profiling
    enable_profiling: AtomicBool,

    /// Syscall timeout (milliseconds, 0 = no timeout)
    syscall_timeout_ms: AtomicUsize,
}

impl PosixConfig {
    /// Create default configuration
    pub const fn new() -> Self {
        Self {
            strict_mode: AtomicBool::new(false),
            max_fds: AtomicUsize::new(1024),
            enable_optimization: AtomicBool::new(true),
            enable_profiling: AtomicBool::new(false),
            syscall_timeout_ms: AtomicUsize::new(0),
        }
    }

    /// Enable strict POSIX compliance mode
    pub fn set_strict_mode(&self, enabled: bool) {
        self.strict_mode.store(enabled, Ordering::Relaxed);
        log::info!(
            "POSIX strict mode: {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if strict mode is enabled
    pub fn is_strict_mode(&self) -> bool {
        self.strict_mode.load(Ordering::Relaxed)
    }

    /// Set maximum file descriptors
    pub fn set_max_fds(&self, max: usize) {
        self.max_fds.store(max, Ordering::Relaxed);
        log::info!("Max FDs per process set to: {}", max);
    }

    /// Get maximum file descriptors
    pub fn get_max_fds(&self) -> usize {
        self.max_fds.load(Ordering::Relaxed)
    }

    /// Enable/disable optimizations
    pub fn set_optimization(&self, enabled: bool) {
        self.enable_optimization.store(enabled, Ordering::Relaxed);

        if enabled {
            crate::posix_x::optimization::enable_all();
        } else {
            crate::posix_x::optimization::disable_all();
        }

        log::info!(
            "POSIX optimizations: {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if optimization is enabled
    pub fn is_optimization_enabled(&self) -> bool {
        self.enable_optimization.load(Ordering::Relaxed)
    }

    /// Enable/disable profiling
    pub fn set_profiling(&self, enabled: bool) {
        self.enable_profiling.store(enabled, Ordering::Relaxed);

        if enabled {
            crate::posix_x::tools::profiler::start_profiling();
        } else {
            crate::posix_x::tools::profiler::stop_profiling();
        }

        log::info!(
            "POSIX profiling: {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if profiling is enabled
    pub fn is_profiling_enabled(&self) -> bool {
        self.enable_profiling.load(Ordering::Relaxed)
    }

    /// Set syscall timeout
    pub fn set_syscall_timeout(&self, timeout_ms: usize) {
        self.syscall_timeout_ms.store(timeout_ms, Ordering::Relaxed);
        log::info!("Syscall timeout set to: {} ms", timeout_ms);
    }

    /// Get syscall timeout
    pub fn get_syscall_timeout(&self) -> usize {
        self.syscall_timeout_ms.load(Ordering::Relaxed)
    }

    /// Load configuration from kernel parameters
    pub fn load_from_cmdline(&self, _cmdline: &str) {
        // Would parse kernel command line
        // Example: posix.strict=1 posix.maxfds=2048
        log::debug!("Loading POSIX config from command line");
    }

    /// Export configuration as string
    pub fn export_config(&self) -> alloc::string::String {
        use alloc::format;

        format!(
            "strict_mode={}, max_fds={}, optimization={}, profiling={}, timeout={}ms",
            self.is_strict_mode(),
            self.get_max_fds(),
            self.is_optimization_enabled(),
            self.is_profiling_enabled(),
            self.get_syscall_timeout()
        )
    }
}

/// Global POSIX configuration
pub static POSIX_CONFIG: PosixConfig = PosixConfig::new();

/// Get global configuration
pub fn get_config() -> &'static PosixConfig {
    &POSIX_CONFIG
}
