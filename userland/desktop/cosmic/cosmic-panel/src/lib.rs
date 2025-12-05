//! # Cosmic Panel
//!
//! Panel/Taskbar component for Cosmic Desktop.

#![no_std]

extern crate alloc;

/// Panel version
pub const VERSION: &str = "0.1.0";

/// Initialize panel
pub fn init() {
    log::info!("Cosmic Panel v{} initializing...", VERSION);
}
