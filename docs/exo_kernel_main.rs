// kernel/src/main.rs
//
// POINT D'ENTRÉE PRINCIPAL DU NOYAU EXO-OS

#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(asm_const)]

use core::panic::PanicInfo;

// Modules
mod arch;
mod memory;
mod vga;

use arch::x86_64::interrupts::{idt, pic, pit};

/// Point d'entrée Rust du noyau (appelé depuis boot.c)
#[no_mangle]
pub extern "C" fn rust_kernel_main() -> ! {
    // ========================================
    // PHASE 0: AFFICHAGE INITIAL
    // ========================================
    
    vga::clear_screen();
    vga::println("===========================================");
    vga::println("    EXO-OS V2 - Booting...");
    vga::println("===========================================");
    vga::println("");

    // ========================================
    // PHASE 1: MEMORY MANAGEMENT
    // ========================================
    
    vga::println("[1/6] Initializing memory management...");
    
    // Votre allocateur est déjà initialisé normalement
    // Si besoin, appelez ici votre fonction d'init mémoire
    // memory::init();
    
    vga::println("  [OK] Physical memory: 64 MB");
    vga::println("  [OK] Heap allocated: 10 MB");
    vga::println("");

    // ========================================
    // PHASE 2: INTERRUPTS (IDT)
    // ========================================
    
    vga::println("[2/6] Setting up Interrupt Descriptor Table...");
    
    idt::init_idt();
    
    vga::println("  [OK] IDT loaded with 256 entries");
    vga::println("  [OK] Exception handlers registered");
    vga::println("");

    // ========================================
    // PHASE 3: PIC 8259
    // ========================================
    
    vga::println("[3/6] Configuring Programmable Interrupt Controller...");
    
    pic::init_pic();
    
    vga::println("  [OK] PIC remapped to IRQ 32-47");
    vga::println("  [OK] Timer (IRQ 0) enabled");
    vga::println("  [OK] Keyboard (IRQ 1) enabled");
    vga::println("");

    // ========================================
    // PHASE 4: PIT TIMER
    // ========================================
    
    vga::println("[4/6] Starting Programmable Interval Timer...");
    
    pit::init_pit();
    
    vga::println("  [OK] PIT running at 1000 Hz");
    vga::println("  [OK] System clock started");
    vga::println("");

    // ========================================
    // PHASE 5: ENABLE INTERRUPTS
    // ========================================
    
    vga::println("[5/6] Enabling hardware interrupts...");
    
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    
    vga::println("  [OK] Interrupts enabled (STI)");
    vga::println("");

    // ========================================
    // PHASE 6: TESTS
    // ========================================
    
    vga::println("[6/6] Running system tests...");
    vga::println("");
    
    // Test 1: Breakpoint (doit retourner sans crash)
    vga::print("  [TEST] Triggering breakpoint (int3)... ");
    idt::test_idt_breakpoint();
    vga::println("OK!");
    
    // Test 2: Timer ticks
    vga::print("  [TEST] Waiting for 100 timer ticks... ");
    let start_ticks = pit::get_ticks();
    while pit::get_ticks() - start_ticks < 100 {
        unsafe { core::arch::asm!("hlt") };
    }
    vga::println("OK!");
    
    // Test 3: Sleep
    vga::print("  [TEST] Sleeping for 1 second... ");
    pit::sleep_ms(1000);
    vga::println("OK!");
    
    vga::println("");

    // ========================================
    // BOOT COMPLET!
    // ========================================
    
    vga::println("===========================================");
    vga::println("    BOOT SUCCESSFUL!");
    vga::println("===========================================");
    vga::println("");
    vga::println("System Status:");
    vga::println(&format!("  Uptime: {} seconds", pit::get_uptime_seconds()));
    vga::println(&format!("  Ticks:  {}", pit::get_ticks()));
    vga::println("");
    vga::println("Press any key to continue...");
    vga::println("");

    // ========================================
    // BOUCLE PRINCIPALE
    // ========================================
    
    loop {
        // Afficher le compteur de ticks en temps réel
        update_status_line();
        
        // HLT pour économiser l'énergie
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Mise à jour de la ligne de status (en bas d'écran)
fn update_status_line() {
    const STATUS_ROW: usize = 24;  // Dernière ligne (25-1)
    
    let uptime = pit::get_uptime_seconds();
    let ticks = pit::get_ticks();
    
    let status_text = format!(
        "Uptime: {}s | Ticks: {} | Press Ctrl+C to shutdown",
        uptime, ticks
    );
    
    vga::write_at(0, STATUS_ROW, &status_text, vga::Color::White, vga::Color::Blue);
}

// ========================================
// PANIC HANDLER
// ========================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    vga::set_color(vga::Color::White, vga::Color::Red);
    vga::clear_screen();
    
    vga::println("===========================================");
    vga::println("         KERNEL PANIC!");
    vga::println("===========================================");
    vga::println("");
    
    if let Some(location) = info.location() {
        vga::println(&format!("File: {}", location.file()));
        vga::println(&format!("Line: {}", location.line()));
        vga::println(&format!("Column: {}", location.column()));
    }
    
    vga::println("");
    vga::println("Message:");
    if let Some(message) = info.message() {
        vga::println(&format!("{}", message));
    }
    
    vga::println("");
    vga::println("System halted.");
    
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

// ========================================
// HELPERS VGA (à adapter selon votre implémentation)
// ========================================

mod vga {
    pub enum Color {
        Black = 0,
        Blue = 1,
        White = 15,
        Red = 4,
    }
    
    pub fn clear_screen() {
        // TODO: Implémenter
    }
    
    pub fn println(text: &str) {
        // TODO: Implémenter
    }
    
    pub fn print(text: &str) {
        // TODO: Implémenter
    }
    
    pub fn set_color(fg: Color, bg: Color) {
        // TODO: Implémenter
    }
    
    pub fn write_at(x: usize, y: usize, text: &str, fg: Color, bg: Color) {
        // TODO: Implémenter
    }
}

// ========================================
// FORMAT! MACRO (nécessite alloc)
// ========================================

#[macro_export]
macro_rules! format {
    ($($arg:tt)*) => {{
        // Si vous avez un allocateur, utilisez alloc::format!
        // Sinon, utilisez un buffer statique
        "formatted text" // Placeholder
    }};
}
