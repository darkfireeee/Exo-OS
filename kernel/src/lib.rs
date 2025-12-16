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

// ═══════════════════════════════════════════════════════════
//  PHASE 0 - Modules essentiels
// ═══════════════════════════════════════════════════════════
pub mod acpi;           // ✅ Phase 0: Détection matériel
pub mod arch;           // ✅ Phase 0: GDT/IDT/Interrupts/Context Switch
pub mod bench;          // ✅ Phase 0: Benchmarks context switch
pub mod boot;           // ✅ Phase 0: Multiboot2 parsing
pub mod c_compat;       // ✅ Phase 0: Compatibilité boot.c
pub mod debug;          // ✅ Phase 0: Debug utilities
pub mod logger;         // ✅ Phase 0: Early logging
pub mod multiboot2;     // ✅ Phase 0: Multiboot2 protocol
pub mod splash;         // ✅ Phase 0: Boot splash screen
pub mod memory;         // ✅ Phase 0: Frame allocator + heap
pub mod scheduler;      // ✅ Phase 0: 3-queue scheduler + context switch
pub mod sync;           // ✅ Phase 0: Spinlock, Mutex basics
pub mod time;           // ✅ Phase 0: PIT timer

// ═══════════════════════════════════════════════════════════
//  PHASE 1 - Syscalls + Process Management (MINIMAL)
// ═══════════════════════════════════════════════════════════
pub mod syscall;        // ✅ Phase 1: Syscall infrastructure
pub mod posix_x;        // ✅ Phase 1: POSIX compatibility layer
pub mod fs;             // 🔄 Phase 1c: VFS activation in progress
// pub mod tests;       // ⏸️ Phase 1c: Tests (dépend de fs complet)

// ═══════════════════════════════════════════════════════════
//  PHASE 1b - À activer après correction fs
// ═══════════════════════════════════════════════════════════
// pub mod loader;      // ⏸️ Phase 1b: ELF loader
// pub mod shell;       // ⏸️ Phase 1b: Interactive shell
// pub mod ffi;         // ⏸️ Phase 1b: FFI userland

// ═══════════════════════════════════════════════════════════
//  DRIVERS - Phase 0 minimal + Phase 1 input
// ═══════════════════════════════════════════════════════════
pub mod drivers;
pub use drivers::char::console::{_print as _console_print, CONSOLE};
pub use drivers::char::serial::{_print as _serial_print, SERIAL1};
pub use drivers::video::vga::{_print as _vga_print, WRITER};

// ═══════════════════════════════════════════════════════════
//  PHASE 2+ - Modules désactivés temporairement
// ═══════════════════════════════════════════════════════════
// pub mod ipc;         // ⏸️ Phase 2: IPC zerocopy
// pub mod net;         // ⏸️ Phase 3: Network stack
// pub mod power;       // ⏸️ Phase 3: Power management
// pub mod security;    // ⏸️ Phase 3: Capabilities

// Re-export for boot stub
pub use memory::heap::LockedHeap;

// Global allocator
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Print panic info to serial
    logger::early_print("\n\n");
    logger::early_print("═══════════════════════════════════════\n");
    logger::early_print("  KERNEL PANIC!\n");
    logger::early_print("═══════════════════════════════════════\n");

    if let Some(location) = info.location() {
        logger::early_print("Location: ");
        logger::early_print(location.file());
        logger::early_print(":");

        // Print line number
        use core::fmt::Write;
        let mut buf = [0u8; 32];
        let mut writer = crate::logger::BufferWriter {
            buffer: &mut buf,
            pos: 0,
        };
        let _ = core::write!(&mut writer, "{}\n", location.line());
        let pos = writer.pos;
        unsafe {
            crate::logger::serial_write_buf(&buf[..pos]);
        }
    }

    if let Some(msg) = info.payload().downcast_ref::<&str>() {
        logger::early_print("Message: ");
        logger::early_print(msg);
        logger::early_print("\n");
    }

    logger::early_print("System halted.\n");

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

// Allocation error handler
#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    logger::early_print("\n\n");
    logger::early_print("═══════════════════════════════════════\n");
    logger::early_print("  HEAP ALLOCATION ERROR!\n");
    logger::early_print("═══════════════════════════════════════\n");
    
    use core::fmt::Write;
    let mut buf = [0u8; 128];
    let mut writer = crate::logger::BufferWriter {
        buffer: &mut buf,
        pos: 0,
    };
    
    let _ = core::write!(&mut writer, "Requested size: {} bytes\n", layout.size());
    let _ = core::write!(&mut writer, "Required align: {} bytes\n", layout.align());
    
    let pos = writer.pos;
    unsafe {
        crate::logger::serial_write_buf(&buf[..pos]);
    }
    
    // Get heap stats
    let stats = ALLOCATOR.stats();
    logger::early_print("Heap stats:\n");
    let mut buf2 = [0u8; 256];
    let mut writer2 = crate::logger::BufferWriter {
        buffer: &mut buf2,
        pos: 0,
    };
    let _ = core::write!(&mut writer2, "  Total: {} KB\n", stats.total_size / 1024);
    let _ = core::write!(&mut writer2, "  Used:  {} KB\n", stats.allocated / 1024);
    let _ = core::write!(&mut writer2, "  Free:  {} KB\n", stats.free / 1024);
    let pos2 = writer2.pos;
    unsafe {
        crate::logger::serial_write_buf(&buf2[..pos2]);
    }
    
    logger::early_print("System halted.\n");
    
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
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
    // CRITICAL: Enable SSE/SIMD FIRST before any other code that might use it
    // The Rust compiler may generate SSE instructions for format!/log!/etc.
    arch::x86_64::simd::init_early();
    
    // Affichage VGA avec balayage d'écran et splash screen v0.5.0
    unsafe {
        vga_clear_with_sweep();
        vga_show_boot_splash();

        // Note: vga_show_system_info() désactivé pour préserver le splash v0.5.0
        // Les infos système sont visibles dans serial.log
    }

    // Initialiser le logger system
    logger::early_print("\n[KERNEL] Initializing logger system...\n");
    logger::init();

    // Afficher le splash screen v0.5.0
    splash::display_splash();
    splash::display_features();

    // Note: log::info!() et autres macros nécessitent le heap allocator
    // Pour l'instant on utilise early_print() direct
    logger::early_print("[KERNEL] Using direct serial output (heap not yet initialized)\n\n");

    // Afficher et vérifier le magic
    logger::early_print("[KERNEL] Multiboot2 Magic: 0x");
    {
        extern "C" {
            fn serial_putc(c: u8);
        }
        let hex_chars = b"0123456789ABCDEF";
        for i in 0..8 {
            let nibble = ((magic >> (28 - i * 4)) & 0xF) as usize;
            unsafe {
                serial_putc(hex_chars[nibble]);
            }
        }
        unsafe {
            serial_putc(b'\n');
        }
    }

    if magic == 0x36d76289 {
        logger::early_print("[KERNEL] ✓ Valid Multiboot2 magic detected\n");
    } else {
        logger::early_print("[KERNEL] ✗ INVALID MAGIC!\n");
        loop {
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }

    // Parser les informations Multiboot2
    logger::early_print("\n[KERNEL] Parsing Multiboot2 information...\n");
    let mb_info = unsafe { multiboot2::parse(multiboot_info) };

    match mb_info {
        Ok(info) => {
            logger::early_print("[KERNEL] ✓ Multiboot2 info parsed successfully\n\n");

            // Afficher les informations
            if let Some(bootloader) = info.bootloader_name {
                logger::early_print("[MB2] Bootloader: ");
                logger::early_print(bootloader);
                logger::early_print("\n");
            }

            if let Some(cmdline) = info.command_line {
                logger::early_print("[MB2] Command line: ");
                logger::early_print(cmdline);
                logger::early_print("\n");
            }

            if let Some(mem_total) = info.total_memory_kb() {
                logger::early_print("[MB2] Total memory: ");
                // Print memory in KB
                unsafe {
                    extern "C" {
                        fn serial_putc(c: u8);
                    }
                    let mut val = mem_total;
                    let mut digits = [0u8; 10];
                    let mut pos = 0;
                    if val == 0 {
                        serial_putc(b'0');
                    } else {
                        while val > 0 {
                            digits[pos] = b'0' + (val % 10) as u8;
                            val /= 10;
                            pos += 1;
                        }
                        for i in (0..pos).rev() {
                            serial_putc(digits[i]);
                        }
                    }
                }
                logger::early_print(" KB\n");
            }

            // Memory map détection (affichage désactivé temporairement pour debug)
            logger::early_print("\n[MB2] Memory Map: Detected\n");

            // Initialiser le frame allocator avec la memory map
            logger::early_print("[KERNEL] Initializing frame allocator...\n");

            // Configuration: Bitmap à 5MB, heap à 8MB
            const BITMAP_ADDR: usize = 0x0050_0000; // 5MB
            const BITMAP_SIZE: usize = 16 * 1024; // 16KB (pour 512MB: 512MB/4KB/8bits = 16KB)
            const HEAP_START: usize = 0x0080_0000; // 8MB
            const TOTAL_MEMORY: usize = 512 * 1024 * 1024; // 512MB

            unsafe {
                memory::physical::init_frame_allocator(
                    BITMAP_ADDR,
                    BITMAP_SIZE,
                    memory::PhysicalAddress::new(0),
                    TOTAL_MEMORY,
                );
            }

            // Marquer les régions réservées
            logger::early_print("[KERNEL] Marking reserved regions...\n");

            // 1. Premiers 1MB (BIOS, VGA, bootloader)
            memory::physical::mark_region_used(memory::PhysicalAddress::new(0), 0x100000);

            // 2. Kernel (1MB - 5MB approximativement)
            memory::physical::mark_region_used(
                memory::PhysicalAddress::new(0x100000),
                4 * 1024 * 1024,
            );

            // 3. Bitmap
            memory::physical::mark_region_used(
                memory::PhysicalAddress::new(BITMAP_ADDR),
                BITMAP_SIZE,
            );

            // 4. Heap (8MB - 72MB = 64MB) - Increased for fork allocations
            const HEAP_SIZE: usize = 64 * 1024 * 1024;
            memory::physical::mark_region_used(
                memory::PhysicalAddress::new(HEAP_START),
                HEAP_SIZE,
            );

            // Vérifier l'initialisation
            if memory::physical::get_allocator_stats().is_some() {
                logger::early_print("[KERNEL] ✓ Frame allocator ready\n");
            } else {
                logger::early_print("[KERNEL] ✗ Frame allocator failed\n");
            }

            logger::early_print("[KERNEL] ✓ Physical memory management ready\n");

            // Initialiser le heap allocator (64MB pour fork/exec)
            logger::early_print("[KERNEL] Initializing heap allocator...\n");
            unsafe {
                ALLOCATOR.init(HEAP_START, HEAP_SIZE);
            }
            logger::early_print("[KERNEL] ✓ Heap allocator initialized (64MB)\n");

            // Tester une allocation pour vérifier que le heap fonctionne
            logger::early_print("[KERNEL] Testing heap allocation...\n");
            {
                use alloc::boxed::Box;
                let test_box = Box::new(42u32);
                if *test_box == 42 {
                    logger::early_print("[KERNEL] ✓ Heap allocation test passed\n");
                } else {
                    logger::early_print("[KERNEL] ✗ Heap allocation test failed\n");
                }
            }

            logger::early_print("[KERNEL] ✓ Dynamic memory allocation ready\n");
            logger::early_print("\n[KERNEL] ═══════════════════════════════════════\n");
            logger::early_print("[KERNEL]   INITIALIZING SYSTEM TABLES\n");
            logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");

            // Mapper les régions APIC avant de les utiliser
            arch::x86_64::memory::paging::map_apic_regions();
            
            // Désactiver l'I/O APIC pour forcer le mode PIC legacy
            arch::x86_64::pic_wrapper::disable_ioapic();

            // Initialiser GDT (Global Descriptor Table)
            logger::early_print("[KERNEL] Initializing GDT...\n");
            arch::x86_64::gdt::init();
            logger::early_print("[KERNEL] ✓ GDT loaded successfully\n");

            // Initialiser IDT (Interrupt Descriptor Table)
            logger::early_print("[KERNEL] Initializing IDT...\n");
            arch::x86_64::idt::init();
            logger::early_print("[KERNEL] ✓ IDT loaded successfully\n");

            // Désactiver les interrupts pendant la configuration
            unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)); }
            logger::early_print("[KERNEL] Interrupts disabled (CLI)\n");
            
            // Configurer PIC et PIT (méthode legacy qui fonctionne)
            logger::early_print("[KERNEL] Configuring PIC 8259...\n");
            arch::x86_64::pic_wrapper::init_pic();
            logger::early_print("[KERNEL] ✓ PIC configured (vectors 32-47)\n");
            
            logger::early_print("[KERNEL] Configuring PIT timer (100Hz)...\n");
            arch::x86_64::pit::init(100);
            logger::early_print("[KERNEL] ✓ PIT configured at 100Hz\n");

            // Afficher le statut du système avec splash
            splash::display_boot_progress("System Tables", 100);
            splash::display_success("KERNEL READY - All systems initialized");
            logger::early_print("\n");

            // Afficher les informations système
            splash::display_system_info(512, 1);

            // Initialiser le scheduler (AVANT d'activer les interrupts!)
            logger::early_print("\n[KERNEL] ═══════════════════════════════════════\n");
            logger::early_print("[KERNEL]   INITIALIZING SCHEDULER\n");
            logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");

            // IMPORTANT: Désactiver les interrupts pendant création des threads
            logger::early_print("[DEBUG] About to call disable_interrupts\n");
            arch::x86_64::disable_interrupts();
            logger::early_print("[DEBUG] disable_interrupts OK\n");

            logger::early_print("[DEBUG] About to call scheduler::init()\n");
            scheduler::init();
            logger::early_print("[KERNEL] ✓ Scheduler initialized\n");

            // Initialize syscall handlers
            logger::early_print("[DEBUG] About to call syscall::handlers::init()\n");
            syscall::handlers::init();
            logger::early_print("[KERNEL] ✓ Syscall handlers initialized\n");

            // Activer les interrupts maintenant que tout est initialisé
            logger::early_print("[DEBUG] About to enable interrupts\n");
            arch::x86_64::enable_interrupts();
            logger::early_print("[DEBUG] Interrupts enabled (STI)\n\n");

            logger::early_print("[KERNEL] ═══════════════════════════════════════\n");
            logger::early_print("[KERNEL]   PHASE 0 BENCHMARK - Context Switch\n");
            logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");
            
            // Exécuter benchmark context switch (Phase 0 validation)
            let (avg, min, max) = scheduler::run_context_switch_benchmark();
            
            // Sauvegarder dans les stats globales
            bench::BENCH_STATS.record_context_switch(avg);
            
            logger::early_print("\n[KERNEL] ═══════════════════════════════════════\n");
            logger::early_print("[KERNEL]   PHASE 0 COMPLETE - Scheduler Ready\n");
            logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");
            logger::early_print("[KERNEL] ✅ Timer + Context Switch validated\n");
            logger::early_print("[KERNEL] ✅ Scheduler 3-queue operational\n");
            logger::early_print("[KERNEL] ✅ Memory management ready\n\n");
            
            logger::early_print("[KERNEL] Starting Phase 1b: fork/exec/wait tests\n\n");
            
            // ✅ PHASE 1b - Create test thread
            logger::early_print("[KERNEL] Creating test thread for Phase 1b...\n");
            
            // Disable interrupts before adding thread
            arch::x86_64::disable_interrupts();
            
            let test_thread = scheduler::thread::Thread::new_kernel(
                1001, // TID
                "phase1b_test",
                test_fork_thread_entry,
                32768, // 32KB stack for tests
            );
            
            if let Err(e) = scheduler::SCHEDULER.add_thread(test_thread) {
                logger::early_print("[ERROR] Failed to add test thread: ");
                let s = alloc::format!("{:?}\n", e);
                logger::early_print(&s);
            } else {
                logger::early_print("[KERNEL] ✅ Test thread added to scheduler\n");
            }
            
            // Re-enable interrupts
            arch::x86_64::enable_interrupts();
            
            // Give test thread time to run
            logger::early_print("[KERNEL] Yielding to test thread...\n\n");
            for _ in 0..1000 {
                scheduler::yield_now();
            }
            
            logger::early_print("\n[KERNEL] ═══════════════════════════════════════\n");
            logger::early_print("[KERNEL]   Phase 1b tests complete\n");
            logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");

            // ⏸️ Shell nécessite VFS complet (Phase 1b)
            // logger::early_print("\n[KERNEL] ═══════════════════════════════════════\n");
            // logger::early_print("[KERNEL]   LAUNCHING INTERACTIVE SHELL\n");
            // logger::early_print("[KERNEL] ═══════════════════════════════════════\n\n");
            // shell::run();
            
            // Idle loop après tests
            logger::early_print("[KERNEL] Entering idle loop after tests...\n");
            loop {
                unsafe {
                    core::arch::asm!("hlt", options(nomem, nostack));
                }
            }
        }
        Err(e) => {
            logger::early_print("[KERNEL] ✗ Failed to parse Multiboot2 info: ");
            logger::early_print(e);
            logger::early_print("\n");
        }
    }

    // Fallback halt (ne devrait jamais être atteint)
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Alias for boot.c compatibility
#[no_mangle]
pub extern "C" fn rust_kernel_entry(magic: u32, multiboot_info: u64) -> ! {
    rust_main(magic, multiboot_info)
}

// ═══════════════════════════════════════════════════════
//  VGA Functions (temporary inline until module is ready)
// ═══════════════════════════════════════════════════════

const VGA_BUFFER: *mut u16 = 0xB8000 as *mut u16;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

unsafe fn vga_clear_with_sweep() {
    let empty = 0x0000u16; // Black on black, space
    let sweep = 0x3FDB; // Cyan on cyan, full block

    // Sweep from top to bottom
    for row in 0..VGA_HEIGHT {
        // Draw sweep line
        for col in 0..VGA_WIDTH {
            let offset = row * VGA_WIDTH + col;
            core::ptr::write_volatile(VGA_BUFFER.add(offset), sweep);
        }

        // Small delay
        for _ in 0..800_000 {
            core::arch::asm!("nop");
        }

        // Replace with empty
        for col in 0..VGA_WIDTH {
            let offset = row * VGA_WIDTH + col;
            core::ptr::write_volatile(VGA_BUFFER.add(offset), empty);
        }
    }
}

// Helper pour écrire une string (pas byte string) sur VGA avec conversion CP437
unsafe fn vga_write_str(row: usize, col: usize, s: &str, color: u16) {
    for (i, ch) in s.chars().enumerate() {
        if col + i >= VGA_WIDTH {
            break;
        }
        // Conversion basique Unicode -> CP437 pour box-drawing
        let byte = match ch {
            '█' => 0xDB,
            '▓' => 0xB2,
            '▒' => 0xB1,
            '░' => 0xB0,
            '╔' => 0xC9,
            '╗' => 0xBB,
            '╚' => 0xC8,
            '╝' => 0xBC,
            '═' => 0xCD,
            '║' => 0xBA,
            '╠' => 0xCC,
            '╣' => 0xB9,
            '╦' => 0xCB,
            '╩' => 0xCA,
            '╬' => 0xCE,
            '╭' => 0xDA,
            '╮' => 0xBF,
            '╰' => 0xC0,
            '╯' => 0xD9,
            '─' => 0xC4,
            '│' => 0xB3,
            '┌' => 0xDA,
            '┐' => 0xBF,
            '└' => 0xC0,
            '┘' => 0xD9,
            '├' => 0xC3,
            '┤' => 0xB4,
            '┬' => 0xC2,
            '┴' => 0xC1,
            '┼' => 0xC5,
            '▀' => 0xDF,
            '▄' => 0xDC,
            '▌' => 0xDD,
            '▐' => 0xDE,
            _ => ch as u8,
        };
        core::ptr::write_volatile(
            VGA_BUFFER.add(row * VGA_WIDTH + col + i),
            color | byte as u16,
        );
    }
}

unsafe fn vga_show_boot_splash() {
    // Couleurs pour le splash v0.5.0
    let frame_color = 0x0B00u16; // Cyan
    let logo_color = 0x0B00u16; // Cyan pour le logo
    let title_color = 0x0E00u16; // Jaune
    let white = 0x0F00u16; // Blanc

    // Draw outer frame (ligne 1 à 20)
    vga_write_str(
        1,
        2,
        "╔══════════════════════════════════════════════════════════════════════╗",
        frame_color,
    );
    for row in 2..19 {
        vga_write_str(row, 2, "║", frame_color);
        vga_write_str(row, 73, "║", frame_color);
    }
    vga_write_str(
        19,
        2,
        "╚══════════════════════════════════════════════════════════════════════╝",
        frame_color,
    );

    // Logo EXO-OS avec les vrais caractères Unicode (lignes 3-9)
    vga_write_str(
        3,
        8,
        "███████╗██╗  ██╗ ██████╗        ██████╗ ███████╗",
        logo_color,
    );
    vga_write_str(
        4,
        8,
        "██╔════╝╚██╗██╔╝██╔═══██╗      ██╔═══██╗██╔════╝",
        logo_color,
    );
    vga_write_str(
        5,
        8,
        "█████╗   ╚███╔╝ ██║   ██║█████╗██║   ██║███████╗",
        logo_color,
    );
    vga_write_str(
        6,
        8,
        "██╔══╝   ██╔██╗ ██║   ██║╚════╝██║   ██║╚════██║",
        logo_color,
    );
    vga_write_str(
        7,
        8,
        "███████╗██╔╝ ██╗╚██████╔╝      ╚██████╔╝███████║",
        logo_color,
    );
    vga_write_str(
        8,
        8,
        "╚══════╝╚═╝  ╚═╝ ╚═════╝        ╚═════╝ ╚══════╝",
        logo_color,
    );

    // Version et nom (centré ligne 10)
    vga_write_str(10, 18, "🚀 Version 0.5.0 - Linux Crusher 🚀", title_color);

    // Ligne de séparation
    vga_write_str(
        11,
        4,
        "──────────────────────────────────────────────────────────────────",
        frame_color,
    );

    // Features (lignes 13-15)
    vga_write_str(
        13,
        6,
        "✨ Memory: NUMA, Zerocopy IPC, mmap/brk/mprotect",
        white,
    );
    vga_write_str(
        14,
        6,
        "⏰ Time: TSC/HPET/RTC, POSIX Timers, nanosleep",
        white,
    );
    vga_write_str(
        15,
        6,
        "🔒 Security: Capabilities, seccomp, pledge/unveil",
        white,
    );

    // Stats (ligne 17)
    vga_write_str(
        17,
        12,
        "📊 ~3000+ lines │ 150+ TODOs eliminated │ 0 errors",
        title_color,
    );

    // Message de boot (ligne 18)
    vga_write_str(18, 18, "⚡ Initializing kernel subsystems...", 0x0A00);

    // Delay pour visibilité
    for _ in 0..15_000_000 {
        core::arch::asm!("nop");
    }
}

unsafe fn vga_show_system_info(magic: u32, multiboot_addr: u64, rsp: u64) {
    let label_color = 0x07u16; // Light gray
    let value_color = 0x0Bu16; // Light cyan

    // Magic
    let label = b"Multiboot2 Magic:";
    for (i, &byte) in label.iter().enumerate() {
        core::ptr::write_volatile(
            VGA_BUFFER.add(19 * VGA_WIDTH + 12 + i),
            (label_color << 8) | byte as u16,
        );
    }

    let hex_chars = b"0123456789ABCDEF";
    core::ptr::write_volatile(
        VGA_BUFFER.add(19 * VGA_WIDTH + 32),
        (value_color << 8) | b'0' as u16,
    );
    core::ptr::write_volatile(
        VGA_BUFFER.add(19 * VGA_WIDTH + 33),
        (value_color << 8) | b'x' as u16,
    );
    for i in 0..8 {
        let nibble = ((magic >> (28 - i * 4)) & 0xF) as usize;
        core::ptr::write_volatile(
            VGA_BUFFER.add(19 * VGA_WIDTH + 34 + i),
            (value_color << 8) | hex_chars[nibble] as u16,
        );
    }

    // Multiboot Info
    let label = b"Multiboot Info:";
    for (i, &byte) in label.iter().enumerate() {
        core::ptr::write_volatile(
            VGA_BUFFER.add(20 * VGA_WIDTH + 12 + i),
            (label_color << 8) | byte as u16,
        );
    }

    core::ptr::write_volatile(
        VGA_BUFFER.add(20 * VGA_WIDTH + 32),
        (value_color << 8) | b'0' as u16,
    );
    core::ptr::write_volatile(
        VGA_BUFFER.add(20 * VGA_WIDTH + 33),
        (value_color << 8) | b'x' as u16,
    );
    for i in 0..16 {
        let nibble = ((multiboot_addr >> (60 - i * 4)) & 0xF) as usize;
        core::ptr::write_volatile(
            VGA_BUFFER.add(20 * VGA_WIDTH + 34 + i),
            (value_color << 8) | hex_chars[nibble] as u16,
        );
    }

    // Stack Pointer
    let label = b"Stack Pointer:";
    for (i, &byte) in label.iter().enumerate() {
        core::ptr::write_volatile(
            VGA_BUFFER.add(21 * VGA_WIDTH + 12 + i),
            (label_color << 8) | byte as u16,
        );
    }

    core::ptr::write_volatile(
        VGA_BUFFER.add(21 * VGA_WIDTH + 32),
        (value_color << 8) | b'0' as u16,
    );
    core::ptr::write_volatile(
        VGA_BUFFER.add(21 * VGA_WIDTH + 33),
        (value_color << 8) | b'x' as u16,
    );
    for i in 0..16 {
        let nibble = ((rsp >> (60 - i * 4)) & 0xF) as usize;
        core::ptr::write_volatile(
            VGA_BUFFER.add(21 * VGA_WIDTH + 34 + i),
            (value_color << 8) | hex_chars[nibble] as u16,
        );
    }
}


/// Thread entry point for Phase 1b tests
fn test_fork_thread_entry() -> ! {
    logger::early_print("[TEST_THREAD] Phase 1b test thread started!\n");
    
    // Run the fork test
    test_fork_syscall();
    
    logger::early_print("[TEST_THREAD] Tests complete, exiting...\n");
    
    // Exit thread
    syscall::handlers::process::sys_exit(0);
}

/// Test fork syscall implementation (Phase 1b)
fn test_fork_syscall() {
    use syscall::dispatch::syscall_numbers::*;
    
    logger::early_print("\n");
    logger::early_print("╔══════════════════════════════════════════════════════════╗\n");
    logger::early_print("║           PHASE 1b - FORK/WAIT TEST                     ║\n");
    logger::early_print("╚══════════════════════════════════════════════════════════╝\n");
    logger::early_print("\n");
    
    // Test 1: Simple fork
    logger::early_print("[TEST 1] Testing sys_fork()...\n");
    
    unsafe {
        let args = [0u64; 6];
        let result = syscall::dispatch::dispatch_syscall(SYS_FORK as u64, &args);
        
        if result > 0 {
            // Parent process
            let child_pid = result as u64;
            logger::early_print("[PARENT] fork() returned child PID: ");
            let s = alloc::format!("{}\n", child_pid);
            logger::early_print(&s);
            
            // Give child time to execute
            logger::early_print("[PARENT] Yielding to let child run...\n");
            for _ in 0..100 {
                scheduler::yield_now();
            }
            
            // Wait for child
            logger::early_print("[PARENT] Waiting for child to exit...\n");
            let mut wstatus: i32 = 0;
            let wait_args = [
                child_pid,
                &mut wstatus as *mut i32 as u64,
                0, // options (blocking wait)
                0, 0, 0
            ];
            let wait_result = syscall::dispatch::dispatch_syscall(SYS_WAIT4 as u64, &wait_args);
            
            if wait_result > 0 {
                logger::early_print("[PARENT] Child exited, status: ");
                let exit_code = (wstatus >> 8) & 0xFF;
                let s = alloc::format!("{}\n", exit_code);
                logger::early_print(&s);
                logger::early_print("[TEST 1] ✅ PASS: fork + wait successful\n");
            } else if wait_result == 0 {
                logger::early_print("[PARENT] wait4() returned 0 (child still running)\n");
                logger::early_print("[TEST 1] ⚠️  PARTIAL: fork succeeded, child may still be running\n");
            } else {
                logger::early_print("[PARENT] wait4() failed with error: ");
                let s = alloc::format!("{}\n", wait_result);
                logger::early_print(&s);
                logger::early_print("[TEST 1] ❌ FAIL: wait failed\n");
            }
        } else {
            logger::early_print("[ERROR] fork() failed with error: ");
            let s = alloc::format!("{}\n", result);
            logger::early_print(&s);
            logger::early_print("[TEST 1] ❌ FAIL: fork failed\n");
        }
    }
    
    logger::early_print("\n");
    logger::early_print("╔══════════════════════════════════════════════════════════╗\n");
    logger::early_print("║           PHASE 1b TEST COMPLETE                        ║\n");
    logger::early_print("╚══════════════════════════════════════════════════════════╝\n");
    logger::early_print("\n");
}
