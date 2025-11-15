//! Point d'entrée binaire du kernel Exo-OS
//! Entry point pour Multiboot2

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

// Importer la bibliothèque kernel
extern crate exo_kernel;

use exo_kernel::c_compat;

/// Point d'entrée du kernel appelé depuis boot.asm
/// 
/// # Arguments
/// * `multiboot_info_addr` - Adresse des informations Multiboot2 (passée par GRUB)
#[no_mangle]
pub extern "C" fn rust_main(multiboot_info_addr: usize) -> ! {
    // DEBUG CRITIQUE: Écrire sur VGA AVANT TOUT
    unsafe {
        let vga = 0xB8000 as *mut u16;
        // Remplir la première ligne avec des 'X' verts
        for i in 0..80 {
            *vga.offset(i) = 0x2F58; // 'X' vert sur noir
        }
    }
    
    // Initialiser le port série COM1 avec le code C
    unsafe {
        c_compat::serial_init();
    }
    
    // Afficher la bannière de démarrage
    print_boot_banner();
    
    // Afficher l'adresse des informations Multiboot
    c_compat::serial_print("\n[BOOT] Multiboot2 info at: 0x");
    print_hex(multiboot_info_addr);
    c_compat::serial_print("\n\n");
    
    c_compat::serial_print("[INFO] All 6 Zero-Copy Fusion phases compiled:\n");
    c_compat::serial_print("  [1] IPC Fusion Rings\n");
    c_compat::serial_print("  [2] Windowed Context Switch\n");
    c_compat::serial_print("  [3] Hybrid Allocator (3 levels)\n");
    c_compat::serial_print("  [4] Predictive Scheduler (EMA)\n");
    c_compat::serial_print("  [5] Adaptive Drivers (4 modes)\n");
    c_compat::serial_print("  [6] Benchmark Framework\n\n");
    
    c_compat::serial_print("[SUCCESS] EXO-OS kernel boot complete!\n");
    c_compat::serial_print("[INFO] Entering infinite loop (minimal kernel)\n\n");
    
    // Boucle infinie (halt)
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Affiche la bannière de démarrage
fn print_boot_banner() {
    c_compat::serial_print("\n\n");
    c_compat::serial_print("========================================\n");
    c_compat::serial_print("    EXO-OS KERNEL v0.2.0-PHASE8-BOOT    \n");
    c_compat::serial_print("========================================\n");
    c_compat::serial_print("Zero-Copy Fusion Architecture\n");
    c_compat::serial_print("Build: WSL Ubuntu + Rust 1.93.0-nightly\n");
    c_compat::serial_print("Target: x86_64-unknown-none (bare-metal)\n");
    c_compat::serial_print("Serial: COM1 0x3F8 @ 38400 baud (C driver)\n");
    c_compat::serial_print("========================================\n");
}

/// Affiche un nombre en hexadécimal (via port série C)
fn print_hex(mut num: usize) {
    let hex_chars = b"0123456789ABCDEF";
    let mut buffer = [0u8; 16];
    let mut i = 0;
    
    if num == 0 {
        unsafe { c_compat::serial_write_char(b'0'); }
        return;
    }
    
    while num > 0 {
        buffer[i] = hex_chars[num & 0xF];
        num >>= 4;
        i += 1;
    }
    
    while i > 0 {
        i -= 1;
        unsafe { c_compat::serial_write_char(buffer[i]); }
    }
}
