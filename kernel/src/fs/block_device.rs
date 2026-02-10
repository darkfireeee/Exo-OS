//! Block Device Abstraction Stub
//!
//! Temporary stub - real implementation in block/device.rs

// Re-export from block module
pub use crate::fs::block::*;

/// Initialize block device subsystem
pub fn init() {
    log::debug!("Block device subsystem stub initialized");
}
