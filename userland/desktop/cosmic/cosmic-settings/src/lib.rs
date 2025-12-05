//! # Cosmic Settings
//!
//! System settings application for Cosmic Desktop.

#![no_std]

extern crate alloc;

/// Settings version
pub const VERSION: &str = "0.1.0";

/// Initialize settings
pub fn init() {
    log::info!("Cosmic Settings v{} initializing...", VERSION);
}
