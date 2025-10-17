// src/lib.rs
// Point d'entrée de la bibliothèque du noyau

#![no_std] // Pas de bibliothèque standard
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt)] // Pour les handlers d'interruptions
#![feature(alloc_error_handler)] // Pour le handler d'erreur d'allocation

// Import de alloc pour les allocations dynamiques
extern crate alloc;

// Modules du noyau
pub mod arch;
pub mod libutils;  // Bibliothèque de modules réutilisables
// pub mod c_compat;  // Module C pour serial et PCI - temporairement désactivé (besoin de clang)
pub mod memory;
pub mod scheduler;
pub mod ipc;
pub mod syscall;
pub mod drivers;

// Réexportation des macros de libutils
pub use libutils::macros::*;

use core::panic::PanicInfo;

/// Macro println! pour l'écriture via le port série
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    SERIAL_WRITER.lock().write_fmt(args).unwrap();
}

/// Writer pour le port série
struct SerialWriter;

impl core::fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Utiliser le pilote serial Rust
        drivers::serial::write_str(s);
        Ok(())
    }
}

use spin::Mutex;
static SERIAL_WRITER: Mutex<SerialWriter> = Mutex::new(SerialWriter);

/// Allocateur global simple utilisant linked_list_allocator
#[global_allocator]
static ALLOCATOR: linked_list_allocator::LockedHeap = linked_list_allocator::LockedHeap::empty();

/// Handler pour les erreurs d'allocation
#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

/// Panic handler pour le noyau
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[KERNEL PANIC] {}", info);
    loop {
        x86_64::instructions::hlt();
    }
}

/// Point d'entrée principal du noyau (appelé depuis main.rs)
#[no_mangle]
pub fn kernel_main(boot_info: &'static bootloader::BootInfo) -> ! {
    // Initialiser le port série en premier pour avoir des logs
    // Initialiser le serial en premier pour les logs
    drivers::serial::init();
    
    println!("===========================================");
    println!("  Exo-OS Kernel v0.1.0");
    println!("  Architecture: x86_64");
    println!("===========================================");
    
    // Afficher les infos du bootloader
    println!("[BOOT] Mémoire physique disponible:");
    let memory_map = &boot_info.memory_map;
    for region in memory_map.iter() {
        println!("  Region: {:?} - Size: {} KB", region.region_type, region.range.end_addr() - region.range.start_addr());
    }
    
    // Initialisation des modules dans l'ordre de dépendance
    println!("[INIT] Architecture x86_64...");
    arch::init(4); // 4 cores par défaut
    
    println!("[INIT] Gestionnaire de mémoire...");
    // Note: memory::init() nécessite des infos du bootloader
    // Pour l'instant on skip, à implémenter plus tard
    // memory::init();
    
    println!("[INIT] Ordonnanceur...");
    scheduler::init(4); // 4 CPUs par défaut
    
    println!("[INIT] IPC...");
    ipc::init();
    
    println!("[INIT] Appels système...");
    syscall::init();
    
    println!("[INIT] Pilotes...");
    drivers::init();
    
    println!("\n[SUCCESS] Noyau initialisé avec succès!\n");
    
    // Test du système
    // TODO: Réactiver quand le code C PCI sera recompilé avec clang
    // println!("[TEST] Enumération PCI...");
    // c_compat::enumerate_pci();
    
    println!("\n[KERNEL] Entrant dans la boucle principale...");
    
    // Boucle principale du noyau
    loop {
        x86_64::instructions::hlt();
    }
}
