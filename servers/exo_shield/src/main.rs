#![no_std]
#![no_main]

// Stub de serveur Exo-OS  implementation a venir.

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
