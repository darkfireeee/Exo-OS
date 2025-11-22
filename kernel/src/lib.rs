//! Exo-OS Kernel Library
//! 
//! Core kernel functionality as a library that can be linked
//! with a boot stub.

#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(const_mut_refs)]
#![allow(dead_code)]
#![allow(unused_imports)]

extern crate alloc;

use core::panic::PanicInfo;

// Public modules
pub mod arch;
pub mod boot;
pub mod c_compat;
pub mod drivers;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod net;
pub mod scheduler;
pub mod syscall;

// Re-export for boot stub
pub use memory::heap::LockedHeap;

// Global allocator
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Try to log panic info if serial is available
    if let Some(location) = info.location() {
        // Placeholder: would use serial output
        let _ = (location.file(), location.line());
    }
    
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

// Allocation error handler
#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// Kernel entry point called from boot stub
#[no_mangle]
pub extern "C" fn _start(_magic: u32, _multiboot_info: *const u8) -> ! {
    // TODO: Initialize architecture
    // TODO: Parse multiboot info
    // TODO: Initialize memory
    // TODO: Initialize heap
    
    // For now, just halt
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
