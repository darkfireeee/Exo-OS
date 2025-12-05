//! Package Manager CLI Entry Point

#![no_std]
#![no_main]

extern crate alloc;

use package_manager::PackageManager;

/// Package manager CLI entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    log::info!("Exo-OS Package Manager v{}", package_manager::VERSION);

    let mut pm = PackageManager::new();

    if let Err(e) = pm.init() {
        log::error!("Failed to initialize package manager: {:?}", e);
    }

    // TODO: Parse CLI arguments and execute commands
    // Commands: install, remove, update, upgrade, search, info, rollback

    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
