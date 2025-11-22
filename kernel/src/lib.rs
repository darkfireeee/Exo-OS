//! Exo-OS Kernel Library
//! 
//! Core kernel functionality as a library that can be linked
//! with a boot stub.

#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(const_mut_refs)]
#![feature(unsafe_attributes)]
#![feature(naked_functions)]
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
    
    // Initialiser l'architecture (IDT simple)
    debug_msg(b"[ARCH] Init IDT...");
    arch::x86_64::init().expect("Failed to initialize architecture");
    debug_msg(b"[ARCH] IDT OK!");
    
    // Test: déclencher un breakpoint pour vérifier le gestionnaire
    debug_msg(b"[TEST] Calling int3...");
    unsafe {
        core::arch::asm!("int3", options(nomem, nostack));
    }
    debug_msg(b"[TEST] int3 OK!");
    
    // Initialiser la mémoire
    debug_msg(b"[MEM] Init memory...");
    let mem_config = memory::MemoryConfig::default_config();
    memory::init(mem_config).expect("Failed to initialize memory");
    debug_msg(b"[MEM] Memory OK!");
    
    // Afficher les stats mémoire
    display_memory_init();
    
    // NE PAS activer les interruptions pour l'instant
    debug_msg(b"[INFO] Timer disabled");
    debug_msg(b"[KERN] Ready!");
    
    // Boucle principale simple sans timer
    kernel_main_loop()
}

/// Affiche la confirmation de l'initialisation mémoire
fn display_memory_init() {
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        let row = 17;
        let msg = b"[RUST] Memory system initialized";
        for (i, &byte) in msg.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0A00 | byte as u16;
        }

        // Afficher les stats si disponible
        if let Some(stats) = memory::physical::get_allocator_stats() {
            let row = 18;
            let msg2 = b"[MEM] Total:     MB  Free:     MB";
            for (i, &byte) in msg2.iter().enumerate() {
                let offset = (row * 80 + i) as isize;
                *vga_buffer.offset(offset) = 0x0700 | byte as u16;
            }

            // Afficher total en MB
            let total_mb = stats.total_memory / (1024 * 1024);
            write_decimal(vga_buffer, row, 13, total_mb as u32);

            // Afficher free en MB
            let free_mb = stats.free_memory / (1024 * 1024);
            write_decimal(vga_buffer, row, 27, free_mb as u32);
        }

        // Afficher les stats du heap
        let heap_stats = ALLOCATOR.stats();
        let row = 19;
        let msg3 = b"[HEAP] Total:     MB  Free:     MB";
        for (i, &byte) in msg3.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0700 | byte as u16;
        }

        let heap_total_mb = heap_stats.total_size / (1024 * 1024);
        write_decimal(vga_buffer, row, 14, heap_total_mb as u32);

        let heap_free_mb = heap_stats.free / (1024 * 1024);
        write_decimal(vga_buffer, row, 28, heap_free_mb as u32);
    }
}

/// Affiche un nombre décimal à l'écran (jusqu'à 3 chiffres)
unsafe fn write_decimal(vga_buffer: *mut u16, row: usize, col: usize, mut value: u32) {
    let mut digits = [b'0'; 3];
    
    for i in (0..3).rev() {
        digits[i] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for (i, &digit) in digits.iter().enumerate() {
        let offset = (row * 80 + col + i) as isize;
        *vga_buffer.offset(offset) = 0x0700 | digit as u16;
    }
}

/// Affiche un message de debug
fn debug_msg(msg: &[u8]) {
    static mut DEBUG_ROW: usize = 20;
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        for (i, &byte) in msg.iter().enumerate() {
            let offset = (DEBUG_ROW * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0E00 | byte as u16; // Jaune
        }
        DEBUG_ROW += 1;
    }
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

/// Boucle principale avec affichage du timer
fn kernel_main_loop_with_timer() -> ! {
    let vga_buffer = 0xB8000 as *mut u16;
    let mut last_second = 0u64;
    
    unsafe {
        let row = 15;
        let msg = b"[KERNEL] Running with timer...";
        for (i, &byte) in msg.iter().enumerate() {
            let offset = (row * 80 + i) as isize;
            *vga_buffer.offset(offset) = 0x0B00 | byte as u16;
        }
    }
    
    loop {
        let uptime_ms = arch::x86_64::pit::get_uptime_ms();
        let current_second = uptime_ms / 1000;
        
        // Mettre à jour l'affichage chaque seconde
        if current_second != last_second {
            last_second = current_second;
            
            unsafe {
                let row = 16;
                let msg = b"[TIME] Uptime:       seconds";
                for (i, &byte) in msg.iter().enumerate() {
                    let offset = (row * 80 + i) as isize;
                    *vga_buffer.offset(offset) = 0x0E00 | byte as u16;
                }
                
                // Afficher le nombre de secondes
                write_decimal(vga_buffer, row, 15, current_second as u32);
            }
        }
        
        // HLT pour économiser le CPU entre les interruptions
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
