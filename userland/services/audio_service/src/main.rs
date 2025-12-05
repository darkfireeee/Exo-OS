//! Audio Service Daemon Entry Point

#![no_std]
#![no_main]

extern crate alloc;

use audio_service::{AudioConfig, AudioService};

/// Audio service main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Initialize logging
    log::info!("Audio service starting...");

    // Create service with default config
    let config = AudioConfig::default();
    let mut service = AudioService::new(config);

    // Start service
    if let Err(e) = service.start() {
        log::error!("Failed to start audio service: {:?}", e);
        // TODO: Exit with error
    }

    log::info!("Audio service running");

    // Main event loop
    loop {
        // TODO: Handle IPC messages via Fusion Ring
        // TODO: Process audio buffers
        // TODO: Handle device hotplug events
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
