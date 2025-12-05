//! # Cosmic Terminal
//!
//! Terminal emulator for Cosmic Desktop.

#![no_std]

extern crate alloc;

/// Terminal version
pub const VERSION: &str = "0.1.0";

/// Initialize terminal
pub fn init() {
    log::info!("Cosmic Terminal v{} initializing...", VERSION);
}
