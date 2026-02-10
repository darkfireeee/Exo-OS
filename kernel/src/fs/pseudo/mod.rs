//! Pseudo Filesystems
//!
//! Special filesystems that expose kernel information and device interfaces:
//! - procfs: Process and system information (/proc)
//! - sysfs: Kernel object hierarchy (/sys)
//! - devfs: Device nodes (/dev)
//!
//! ## Architecture
//! - Dynamic content generation
//! - Read-only for most files (some writable in sysfs)
//! - Minimal memory footprint
//! - Real-time data reflection
//!
//! ## Performance Targets
//! - File lookup: < 100ns (hash-based)
//! - Read operations: < 1μs for small files
//! - Directory listing: < 10μs

pub mod procfs;
pub mod sysfs;
pub mod devfs;

pub use procfs::ProcFs;
pub use sysfs::SysFs;
pub use devfs::DevFs;

/// Initialize process filesystem
pub fn init_procfs() {
    procfs::init();
    log::info!("✓ ProcFS initialized at /proc");
}

/// Initialize system filesystem
pub fn init_sysfs() {
    sysfs::init();
    log::info!("✓ SysFS initialized at /sys");
}

/// Initialize device filesystem
pub fn init_devfs() {
    devfs::init();
    log::info!("✓ DevFS initialized at /dev");
}

/// Initialize all pseudo filesystems
pub fn init() {
    log::info!("Initializing pseudo filesystems...");
    init_procfs();
    init_sysfs();
    init_devfs();
    log::info!("✓ Pseudo filesystems initialized");
}
