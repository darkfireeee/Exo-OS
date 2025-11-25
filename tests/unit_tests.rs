#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(exo_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate exo_os;

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    test_main();
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    exo_os::test_panic_handler(info)
}

mod unit {
    pub mod utils_test;
}
