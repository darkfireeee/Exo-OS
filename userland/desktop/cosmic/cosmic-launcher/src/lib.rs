//! # Cosmic Launcher
//!
//! Application launcher for Cosmic Desktop.

#![no_std]

extern crate alloc;

/// Launcher version
pub const VERSION: &str = "0.1.0";

/// Initialize launcher
pub fn init() {
    log::info!("Cosmic Launcher v{} initializing...", VERSION);
}
