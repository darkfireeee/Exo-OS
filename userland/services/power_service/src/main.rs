//! Power Service Daemon Entry Point

#![no_std]
#![no_main]

extern crate alloc;

use power_service::PowerService;

/// Power service main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    log::info!("Power service starting...");

    let mut service = PowerService::new();

    if let Err(e) = service.start() {
        log::error!("Failed to start power service: {:?}", e);
    }

    log::info!("Power service running");

    // Main event loop
    loop {
        // TODO: Handle IPC messages
        // TODO: Monitor battery/AC events
        // TODO: Adjust profile as needed
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
