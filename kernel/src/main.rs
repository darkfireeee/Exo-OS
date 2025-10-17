// src/main.rs// src/main.rs

// Point d'entrée binaire du kernel Exo-OS// Binaire principal du noyau Exo-OS



#![no_std]#![no_std] // Pas de bibliothèque standard

#![no_main]#![no_main] // Pas de point d'entrée standard

#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;#![feature(alloc_error_handler)]

use exo_kernel::{println, arch, c_compat};

extern crate alloc;

#[panic_handler]

fn panic(info: &PanicInfo) -> ! {use core::panic::PanicInfo;

    println!("[PANIC] {}", info);

    loop {// Modules du kernel (inline dans le binaire)

        x86_64::instructions::hlt();#[path = "lib.rs"]

    }mod kernel_lib;

}

use kernel_lib::*;

#[no_mangle]

pub extern "C" fn _start() -> ! {/// Cette fonction est appelée en cas de panic.

    // Initialiser le port série#[panic_handler]

    unsafe {fn panic(info: &PanicInfo) -> ! {

        c_compat::serial_init();    println!("[PANIC] {}", info);

    }    loop {

            x86_64::instructions::hlt();

    println!("========================================");    }

    println!("   Exo-OS Kernel v0.1.0");}

    println!("========================================");

    println!();#[no_mangle]

    pub extern "C" fn _start() -> ! {

    println!("[INIT] Initialisation de l'architecture x86_64...");    // Initialiser le port série en premier

    arch::init(4);    unsafe {

            c_compat::serial_init();

    println!("[INIT] Configuration des interruptions...");    }

    arch::interrupts::init();    

    arch::interrupts::enable();    // Point d'entrée du kernel

        println!("========================================");

    println!("[OK] Kernel initialisé avec succès!");    println!("   Exo-OS Kernel v0.1.0");

    println!();    println!("========================================");

    println!("[IDLE] Kernel en attente d'interruptions...");    

        // Initialisation du kernel

    loop {    println!("[INIT] Initialisation de l'architecture x86_64...");

        x86_64::instructions::hlt();    arch::init(4); // 4 cores pour le test

    }    

}    println!("[INIT] Initialisation des interruptions...");

    arch::interrupts::enable();
    
    println!("[OK] Kernel initialisé avec succès!");
    println!("");
    println!("[IDLE] Kernel en attente...");
    
    // Boucle principale du kernel
    loop {
        arch::halt();
    }
}