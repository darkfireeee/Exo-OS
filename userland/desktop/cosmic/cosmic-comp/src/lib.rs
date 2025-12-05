//! # Cosmic Compositor
//!
//! Wayland compositor for Exo-OS using Fusion Rings IPC.

#![no_std]

extern crate alloc;

/// Compositor version
pub const VERSION: &str = "0.1.0";

/// Initialize compositor
pub fn init() {
    log::info!("Cosmic Compositor v{} initializing...", VERSION);
}
