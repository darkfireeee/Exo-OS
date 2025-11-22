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

/// Point d'entrée du kernel Rust (appelé depuis kernel_stub.c)
/// 
/// À ce stade:
/// - Le CPU est en mode 64-bit
/// - Le paging est configuré (identity mapped)
/// - La stack est configurée
/// - VGA text mode est initialisé
#[no_mangle]
pub extern "C" fn rust_main(magic: u32, multiboot_info: u64) -> ! {
    // Afficher message de bienvenue Rust
    rust_welcome(magic, multiboot_info);
    
    // TODO: Initialize architecture
    // arch::init().expect("Failed to initialize architecture");
    
    // TODO: Parse multiboot info
    // let mboot = boot::parse_multiboot(multiboot_info);
    
    // TODO: Initialize memory
    // memory::init(&mboot).expect("Failed to initialize memory");
    
    // TODO: Initialize heap
    // unsafe { ALLOCATOR.init(heap_start, heap_size); }
    
    // TODO: Initialize scheduler
    // scheduler::init();
    
    // Boucle principale du kernel
    kernel_main_loop()
}

/// Affiche un message de bienvenue depuis le code Rust
fn rust_welcome(magic: u32, mboot_info: u64) {
    // Buffer VGA à 0xB8000
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        // Ligne 12: Message Rust
        let row = 12;
        let col = 0;
        let msg = b"[RUST] Rust kernel initialized!";
        let color = 0x0A00; // Vert clair
        
        for (i, &byte) in msg.iter().enumerate() {
            let offset = (row * 80 + col + i) as isize;
            *vga_buffer.offset(offset) = color | byte as u16;
        }
        
        // Ligne 13: Afficher magic et mboot info
        let row = 13;
        let msg2 = b"[RUST] Magic: 0x        MBoot: 0x";
        for (i, &byte) in msg2.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0700 | byte as u16;
        }
        
        // Afficher magic en hexa (simplifié)
        write_hex(vga_buffer, row, 18, magic);
        write_hex(vga_buffer, row, 35, mboot_info as u32);
    }
}

/// Affiche un nombre en hexadécimal dans le buffer VGA
unsafe fn write_hex(vga_buffer: *mut u16, row: usize, col: usize, value: u32) {
    const HEX_CHARS: &[u8] = b"0123456789ABCDEF";
    
    for i in 0..8 {
        let nibble = ((value >> ((7 - i) * 4)) & 0xF) as usize;
        let offset = (row * 80 + col + i) as isize;
        *vga_buffer.offset(offset) = 0x0700 | HEX_CHARS[nibble] as u16;
    }
}

/// Boucle principale du kernel
fn kernel_main_loop() -> ! {
    // Pour l'instant, juste halter avec des messages de debug
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        let row = 15;
        let msg = b"[RUST] Entering main kernel loop...";
        for (i, &byte) in msg.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0B00 | byte as u16;
        }
        
        let row = 16;
        let msg2 = b"[RUST] System idle - HLT loop active";
        for (i, &byte) in msg2.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0800 | byte as u16;
        }
    }
    
    // Boucle infinie avec HLT
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
