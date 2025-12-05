//! # Cosmic Files
//!
//! File manager for Cosmic Desktop.

#![no_std]

extern crate alloc;

/// Files version
pub const VERSION: &str = "0.1.0";

/// Initialize file manager
pub fn init() {
    log::info!("Cosmic Files v{} initializing...", VERSION);
}
