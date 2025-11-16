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

/// Allocateur global hybride (fallback: linked_list_allocator)
#[cfg(not(feature = "hybrid_allocator"))]
#[global_allocator]
static ALLOCATOR: linked_list_allocator::LockedHeap = linked_list_allocator::LockedHeap::empty();

#[cfg(feature = "hybrid_allocator")]
#[global_allocator]
static ALLOCATOR: memory::hybrid_allocator::HybridAllocator = memory::hybrid_allocator::HybridAllocator::new();

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
    // Mesure du temps de boot (cycles CPU)
    let boot_start_cycles = perf_counters::rdtsc();
    // Initialiser le port série en premier pour avoir des logs
    drivers::serial::init();
    
    // Traces très précoces pour diagnostiquer les blocages observés ("64SCPP")
    println!("[DBG] kernel_main(): args: magic=0x{:x}, info=0x{:x}", multiboot_magic, multiboot_info_ptr);

    println!("===========================================");
    println!("  Exo-OS Kernel v0.2.0-PHASE8-BOOT");
    println!("  Architecture: x86_64");
    println!("  Bootloader: Multiboot2 + GRUB");
    println!("===========================================");
    // Affichage VGA convivial dès le début pour la fenêtre QEMU/VM
    libutils::display::write_banner();
    
    // Vérifier le magic number multiboot2
    if multiboot_magic != 0x36d76289 {
        println!("[ERROR] Multiboot2 magic invalide: 0x{:x}", multiboot_magic);
        panic!("Invalid multiboot2 magic number: 0x{:x}", multiboot_magic);
    }
    
    println!("[BOOT] Multiboot2 magic validé: 0x{:x}", multiboot_magic);
    println!("[BOOT] Multiboot info @ 0x{:x}", multiboot_info_ptr);
    println!("[DBG] Chargement des tags Multiboot2...");
    
    // Parser les informations multiboot2 avec la nouvelle API
    let boot_info = unsafe {
        use multiboot2::{BootInformationHeader, BootInformation};
        match BootInformation::load(multiboot_info_ptr as *const BootInformationHeader) {
            Ok(bi) => {
                println!("[DBG] BootInformation::load OK");
                bi
            },
            Err(_e) => {
                println!("[ERROR] BootInformation::load a échoué (ptr=0x{:x}). Probable #PF (zones non mappées)", multiboot_info_ptr);
                println!("[HINT] Vérifiez le mapping des pages initial (p2_table: 1GiB identity map)");
                loop { x86_64::instructions::hlt(); }
            }
        }
    };
    
    // Afficher les informations de la mémoire et initialiser le heap
    let mut heap_initialized = false;
    let mut total_usable_mb: u64 = 0;
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
                        #[cfg(not(feature = "hybrid_allocator"))]
                        {
                            ALLOCATOR.lock().init(heap_start as *mut u8, heap_size);
                        }
                        #[cfg(feature = "hybrid_allocator")]
                        {
                            ALLOCATOR.init_fallback(heap_start as *mut u8, heap_size);
                        }
                    }
                    println!("[MEMORY] Heap initialisé: 0x{:x} - 0x{:x} ({} KB)", heap_start, heap_start + heap_size, heap_size / 1024);
                    heap_initialized = true;
                }
            }
        }
        println!("\n  {} régions mémoire utilisables", region_count);
        total_usable_mb = total_usable / 1024 / 1024;
        println!("  Mémoire utilisable totale: {} MB", total_usable_mb);
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
    let scheduler_ok = true;
    // Threads de démonstration préemptifs (faible verbosité sous NEM)
    fn demo_a() {
        let mut n: u64 = 0;
        loop {
            if n % 200 == 0 { println!("[demo A]"); }
            n = n.wrapping_add(1);
            // Travail simulé
            for _ in 0..2000 { unsafe { core::arch::asm!("nop") }; }
            // Laisser le timer préempter (pas de yield coopératif)
        }
    }
    fn demo_b() {
        let mut n: u64 = 0;
        loop {
            if n % 200 == 0 { println!("[demo B]"); }
            n = n.wrapping_add(1);
            for _ in 0..1500 { unsafe { core::arch::asm!("nop") }; }
            // Préemption uniquement via timer
        }
    }
    // Désactivé pour stabilité des microbenchmarks (éviter les switches précoces)
    // println!("[INIT] Spawn demo threads...");
    // scheduler::spawn(demo_a, Some("demo_a"), None);
    // scheduler::spawn(demo_b, Some("demo_b"), None);
    
    println!("[INIT] IPC...");
    ipc::init();
    let ipc_ok = true;
    
    println!("[INIT] Appels système...");
    syscall::init();

    // Déclencheurs de performance au boot pour alimenter les métriques (désactivés pour stabilité sous Fusion Rings)
    // 1) Générer quelques événements IPC + Syscall (voie standard)
    if false {
        use crate::syscall::SyscallArgs;
        // Créer un canal de test dédié
        let test_channel_id = ipc::create_channel("perf_test", 64).unwrap_or(1);

        // Préparer un petit message
        let msg: [u8; 4] = *b"ping";

        // Appel système d'envoi IPC
        let send_args = SyscallArgs {
            rdi: test_channel_id as u64,   // canal
            rsi: msg.as_ptr() as u64,      // pointeur données
            rdx: msg.len() as u64,         // taille
            r10: 0,
            r8: 0,
            r9: 0,
        };
        let _ = syscall::sys_ipc_send(send_args);

        // Buffer de réception
        let mut recv_buf = [0u8; 16];
        let recv_args = SyscallArgs {
            rdi: test_channel_id as u64,
            rsi: recv_buf.as_mut_ptr() as u64,
            rdx: recv_buf.len() as u64,
            r10: 0,
            r8: 0,
            r9: 0,
        };
        let _ = syscall::sys_ipc_recv(recv_args);
    }

    // 2) Si Fusion Rings est activé, déclencher le fast path
    #[cfg(feature = "fusion_rings")]
    if false {
        let _ = ipc::send_fast("log", b"boot-ok");
        let _ = ipc::receive_fast("log");
    }
    
    println!("[INIT] Pilotes...");
    drivers::init();
    
    // Initialiser le système de mesure de performance
    println!("[PERF] Initialisation des compteurs de performance...");
    perf_counters::PERF_MANAGER.reset();
    println!("[PERF] Système de performance initialisé.");
    
    // Afficher le statut sur VGA sous la bannière (mémoire + OK)
    libutils::display::write_boot_status(total_usable_mb, heap_initialized, scheduler_ok, ipc_ok);

    // Enregistrer la durée du boot noyau jusqu'ici
    let boot_end_cycles = perf_counters::rdtsc();
    perf_counters::PERF_MANAGER.record(perf_counters::Component::KernelBoot, boot_end_cycles - boot_start_cycles);
    
    println!("\n[SUCCESS] Noyau initialisé avec succès!\n");
    
    // Bannière déjà affichée plus haut (éviter de nettoyer l'écran à nouveau)

    // Petit délai pour laisser passer quelques ticks timer et ordonnancements
    for _ in 0..500_000 { unsafe { core::arch::asm!("nop") } }

    // Déclencheurs de performance APRÈS reset (désactivés pendant mise au point)
    if false {
        use crate::syscall::SyscallArgs;
        // S'assurer que le canal de test existe
        let test_channel_id = ipc::create_channel("perf_test", 64).unwrap_or(1);
        // Envoi/réception via appels système (mesure Syscall + IPC instrumenté)
        let msg: [u8; 4] = *b"pong";
        let send_args = SyscallArgs { rdi: test_channel_id as u64, rsi: msg.as_ptr() as u64, rdx: msg.len() as u64, r10: 0, r8: 0, r9: 0 };
        let _ = syscall::sys_ipc_send(send_args);
        let mut recv_buf = [0u8; 16];
        let recv_args = SyscallArgs { rdi: test_channel_id as u64, rsi: recv_buf.as_mut_ptr() as u64, rdx: recv_buf.len() as u64, r10: 0, r8: 0, r9: 0 };
        let _ = syscall::sys_ipc_recv(recv_args);
    }

    #[cfg(feature = "fusion_rings")]
    if false {
        let _ = ipc::send_fast("log", b"boot-metrics");
        let _ = ipc::receive_fast("log");
    }

    println!("\n[KERNEL] Entrant dans la boucle principale...");
    
    // Microbenchmarks runtime (IPC inline, Syscall roundtrip)
    crate::perf::runtime_bench::run_startup_microbenchmarks();

    // Afficher un rapport de performance au démarrage
    crate::perf_counters::print_summary_report();
    
    // Boucle principale du noyau
    loop {
        x86_64::instructions::hlt();
    }
}
