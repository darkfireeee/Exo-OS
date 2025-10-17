//! Point d'entrée binaire du kernel Exo-OS

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader::BootInfo;

#[no_mangle]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    exo_kernel::kernel_main(boot_info);
}
