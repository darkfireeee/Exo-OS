//! Exo-OS Bootloader
//! Simple bootloader stub - uses bootloader crate

#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Bootloader entry point
    // The real bootloader functionality is provided by the bootloader crate
    loop {}
}
