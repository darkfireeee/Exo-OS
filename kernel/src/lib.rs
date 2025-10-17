// src/lib.rs
// Point d'entrée de la bibliothèque du noyau

#![no_std] // Pas de bibliothèque standard
#![cfg_attr(test, no_main)]

// Modules du noyau
pub mod arch;
pub mod c_compat;
pub mod memory;
pub mod scheduler;
pub mod ipc;
pub mod syscall;
pub mod drivers;

use core::panic::PanicInfo;

/// Cette fonction est appelée en cas de panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

/// Point d'entrée principal du noyau Rust
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    println!("Exo-OS Kernel v0.1.0 (from Rust)");
    
    // Initialisation des modules
    arch::init();
    c_compat::init();
    memory::init();
    scheduler::init();
    ipc::init();
    syscall::init();
    drivers::init();
    
    println!("Noyau initialisé avec succès.");
    
    // Boucle principale du noyau
    loop {}
}