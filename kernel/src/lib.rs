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
pub mod c_compat;  // Module C pour serial (compilé dans build.rs)
pub mod memory;
pub mod scheduler;
pub mod ipc;
pub mod syscall;
pub mod drivers;
pub mod perf_counters;  // Module de mesure de performance
pub mod perf;  // Phase 6: Framework de benchmarking unifié

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
#[cfg(not(test))]
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

/// Point d'entrée principal du noyau (appelé depuis bootloader/boot.asm)
/// 
/// Arguments passés depuis l'assembleur :
/// - RDI : pointeur vers la structure d'information multiboot2
/// - RSI : magic number multiboot2 (0x36d76289)
#[no_mangle]
pub extern "C" fn kernel_main(multiboot_info_ptr: u64, multiboot_magic: u32) -> ! {
    // Initialiser le port série en premier pour avoir des logs
    drivers::serial::init();
    
    println!("===========================================");
    println!("  Exo-OS Kernel v0.2.0-PHASE8-BOOT");
    println!("  Architecture: x86_64");
    println!("  Bootloader: Multiboot2 + GRUB");
    println!("===========================================");
    
    // Vérifier le magic number multiboot2
    if multiboot_magic != 0x36d76289 {
        panic!("Invalid multiboot2 magic number: 0x{:x}", multiboot_magic);
    }
    
    println!("[BOOT] Multiboot2 magic validé: 0x{:x}", multiboot_magic);
    println!("[BOOT] Multiboot info @ 0x{:x}", multiboot_info_ptr);
    
    // Parser les informations multiboot2 avec la nouvelle API
        let boot_info = unsafe {
            use multiboot2::{BootInformationHeader, BootInformation};
            BootInformation::load(multiboot_info_ptr as *const BootInformationHeader)
                .expect("Failed to load multiboot2 information")
        };
    
    // Afficher les informations de la mémoire et initialiser le heap
    let mut heap_initialized = false;
    if let Some(memory_map_tag) = boot_info.memory_map_tag() {
        let mut total_usable = 0u64;
        let mut region_count = 0;
        println!("\n[MEMORY] Carte mémoire:");
        for area in memory_map_tag.memory_areas() {
            let start: u64 = area.start_address();
            let end: u64 = area.end_address();
            let size = end - start;
            if area.typ() == multiboot2::MemoryAreaType::Available {
                total_usable += size;
                region_count += 1;
                println!("  0x{:016x} - 0x{:016x} ({} MB) [Disponible]", start, end, size / 1024 / 1024);
                // Initialise le heap sur la première région disponible
                if !heap_initialized && size > 1024 * 1024 {
                    use core::ptr::NonNull;
                    let heap_start = start as usize + 0x10000; // Laisse 64K pour le boot
                    let heap_size = (size as usize).saturating_sub(0x10000).min(16 * 1024 * 1024); // max 16 MiB
                    unsafe {
                        ALLOCATOR.lock().init(heap_start as *mut u8, heap_size);
                    }
                    println!("[MEMORY] Heap initialisé: 0x{:x} - 0x{:x} ({} KB)", heap_start, heap_start + heap_size, heap_size / 1024);
                    heap_initialized = true;
                }
            }
        }
        println!("\n  {} régions mémoire utilisables", region_count);
        println!("  Mémoire utilisable totale: {} MB", total_usable / 1024 / 1024);
        if !heap_initialized {
            println!("[WARNING] Heap non initialisé: aucune région mémoire disponible suffisante");
        }
    } else {
        println!("[WARNING] Pas de carte mémoire disponible");
    }
    
    // Afficher les informations sur les modules chargés
    let mut has_modules = false;
    for module in boot_info.module_tags() {
        if !has_modules {
            println!("\n[BOOT] Modules chargés:");
            has_modules = true;
        }
        println!("  Module @ 0x{:x} - 0x{:x}", 
            module.start_address(), module.end_address());
        if let Ok(name) = module.cmdline() {
            println!("    Nom: {}", name);
        }
    }
    
    // Afficher les informations du bootloader
    if let Some(boot_loader_name_tag) = boot_info.boot_loader_name_tag() {
        if let Ok(name) = boot_loader_name_tag.name() {
            println!("\n[BOOT] Bootloader: {}", name);
        }
    }
    
    // Initialisation des modules dans l'ordre de dépendance
    println!("\n[INIT] Architecture x86_64...");
    arch::init(4); // 4 cores par défaut
    
    println!("[INIT] Gestionnaire de mémoire...");
    memory::init(&boot_info);
    // Auto-test heap après init mémoire
    memory::heap_allocator::selftest();
    
    println!("[INIT] Ordonnanceur...");
    scheduler::init(4); // 4 CPUs par défaut
    // Threads de démonstration préemptifs
    fn demo_a() {
        loop {
            println!("[demo A] tick");
            // Simule un travail puis cède volontairement parfois
            for _ in 0..10 { unsafe { core::arch::asm!("nop") }; }
            // Cooperative yield pour voir l'alternance même sans timer
            crate::scheduler::yield_();
        }
    }
    fn demo_b() {
        loop {
            println!("[demo B] tick");
            for _ in 0..5 { unsafe { core::arch::asm!("nop") }; }
            crate::scheduler::yield_();
        }
    }
    println!("[INIT] Spawn demo threads...");
    scheduler::spawn(demo_a, Some("demo_a"), None);
    scheduler::spawn(demo_b, Some("demo_b"), None);
    
    println!("[INIT] IPC...");
    ipc::init();
    
    println!("[INIT] Appels système...");
    syscall::init();
    
    println!("[INIT] Pilotes...");
    drivers::init();
    
    // Initialiser le système de mesure de performance
    println!("[PERF] Initialisation des compteurs de performance...");
    perf_counters::PERF_MANAGER.reset();
    println!("[PERF] Système de performance initialisé.");
    
    println!("\n[SUCCESS] Noyau initialisé avec succès!\n");
    
    // Affichage visuel sur VGA pour confirmer que le noyau est actif
    println!("[DISPLAY] Écriture du banner VGA...");
    libutils::display::write_banner();
    println!("[DISPLAY] Banner VGA écrit avec succès");

    println!("\n[KERNEL] Entrant dans la boucle principale...");
    
    // Afficher un rapport de performance au démarrage
    crate::perf_counters::print_summary_report();
    
    // Boucle principale du noyau
    loop {
        x86_64::instructions::hlt();
    }
}
