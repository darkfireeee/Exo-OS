// src/main.rs
// Binaire principal du noyau Exo-OS

#![no_std] // Pas de bibliothèque standard
#![no_main] // Pas de point d'entrée standard
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::panic::PanicInfo;

// Modules du kernel (inline dans le binaire)
#[path = "lib.rs"]
mod kernel_lib;

use kernel_lib::*;

/// Cette fonction est appelée en cas de panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[PANIC] {}", info);
    loop {
        x86_64::instructions::hlt();
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Initialiser le port série en premier
    unsafe {
        c_compat::serial_init();
    }
    
    // Point d'entrée du kernel
    println!("========================================");
    println!("   Exo-OS Kernel v0.1.0");
    println!("========================================");
    
    // Initialisation du kernel
    println!("[INIT] Initialisation de l'architecture x86_64...");
    arch::init(4); // 4 cores pour le test
    
    println!("[INIT] Initialisation des interruptions...");
    arch::interrupts::enable();
    
    println!("[OK] Kernel initialisé avec succès!");
    println!("");
    println!("[IDLE] Kernel en attente...");
    
    // Boucle principale du kernel
    loop {
        arch::halt();
    }
}