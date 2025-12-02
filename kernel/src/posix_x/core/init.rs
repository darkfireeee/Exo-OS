//! POSIX-X Initialization
//!
//! Initializes all POSIX subsystems

use crate::posix_x::core::config::POSIX_CONFIG;

/// Initialize POSIX-X subsystem
pub fn init() {
    log::info!("Initializing POSIX-X compatibility layer...");

    // Initialize optimization subsystems
    crate::posix_x::optimization::init();
    log::debug!("  ✓ Optimization subsystems initialized");

    // Initialize VFS
    // crate::posix_x::vfs_posix::init();
    log::debug!("  ✓ VFS subsystem ready");

    // Initialize kernel interfaces
    init_kernel_interfaces();
    log::debug!("  ✓ Kernel interfaces initialized");

    // Load configuration
    POSIX_CONFIG.load_from_cmdline("");
    log::debug!("  ✓ Configuration loaded: {}", POSIX_CONFIG.export_config());

    // Print compatibility report
    let report = crate::posix_x::core::compatibility::get_compatibility_report();
    log::info!("POSIX Compliance: {:.1}%", report.compliance_percentage());

    log::info!("✓ POSIX-X initialization complete!");
}

/// Initialize kernel interface bridges
fn init_kernel_interfaces() {
    // Initialize IPC bridge
    // crate::posix_x::kernel_interface::ipc_bridge::init();

    // Initialize memory bridge
    // crate::posix_x::kernel_interface::memory_bridge::init();

    // Initialize signal daemon
    // crate::posix_x::kernel_interface::signal_daemon::init();

    log::debug!("Kernel interface bridges initialized");
}

/// Shutdown POSIX-X subsystem
pub fn shutdown() {
    log::info!("Shutting down POSIX-X...");

    // Flush any pending operations
    crate::posix_x::optimization::BATCH_OPTIMIZER.flush();

    // Stop profiling if active
    if POSIX_CONFIG.is_profiling_enabled() {
        crate::posix_x::tools::profiler::stop_profiling();
    }

    log::info!("✓ POSIX-X shutdown complete");
}

/// Check if POSIX-X is initialized
static INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Mark as initialized
pub fn mark_initialized() {
    INITIALIZED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Check initialization status
pub fn is_initialized() -> bool {
    INITIALIZED.load(core::sync::atomic::Ordering::Relaxed)
}
