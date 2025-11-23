//! Architecture x86_64 support for Exo-OS
//! 
//! Ce module gère les spécificités de l'architecture x86_64:
//! - GDT (Global Descriptor Table) pour la segmentation
//! - IDT (Interrupt Descriptor Table) pour les interruptions
//! - Gestion des interruptions et exceptions CPU
//! - Configuration des registres de contrôle

pub mod handlers;  // Interrupt handlers with correct stack alignment
pub mod idt;
pub mod pic;  // Programmable Interrupt Controller (old implementation)
pub mod pic_wrapper;  // NEW: PIC wrapper using pic8259 crate
pub mod pit;  // Programmable Interval Timer
pub mod io_diagnostic;  // NEW: I/O privilege diagnostic tools

// Constantes d'architecture
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_2MB: usize = 2 * 1024 * 1024;
pub const PAGE_SIZE_1GB: usize = 1024 * 1024 * 1024;

// Layout mémoire du kernel (identity mapped pour l'instant)
pub const KERNEL_PHYSICAL_BASE: usize = 0x0010_0000;        // 1MB (après GRUB)
pub const KERNEL_VIRTUAL_BASE: usize = 0x0010_0000;         // Identity mapped
pub const KERNEL_STACK_SIZE: usize = 16 * 1024;             // 16KB

// Constantes legacy pour compatibilité avec le code existant
pub const KERNEL_START_ADDRESS: usize = 0xFFFF_8000_0000_0000;
pub const KERNEL_END_ADDRESS: usize = 0xFFFF_FFFF_FFFF_FFFF;
pub const KERNEL_VIRTUAL_OFFSET: usize = 0xFFFF_8000_0000_0000;
pub const KERNEL_CODE_START: usize = 0xFFFF_8000_0010_0000;
pub const KERNEL_CODE_END: usize = 0xFFFF_8000_0020_0000;
pub const KERNEL_BASE: usize = 0xFFFF_8000_0000_0000;

// Adresses importantes
pub const VGA_BUFFER_ADDR: usize = 0xB8000;
pub const VGA_BUFFER_SIZE: usize = 80 * 25 * 2;

/// Initialise l'architecture x86_64
/// 
/// Cette fonction doit être appelée au démarrage du kernel.
/// Elle configure:
/// - La GDT (déjà configurée par le bootloader mais peut être reconfigurée)
/// - L'IDT (table des interruptions)
/// - Les interruptions CPU
pub fn init() -> Result<(), &'static str> {
    // Pour l'instant, le bootloader a déjà configuré la GDT
    
    // Initialiser l'IDT avec tous les handlers
    idt::init();
    
    // Note: On n'active PAS les interruptions ici car le PIC n'est pas encore configuré
    // Les interruptions seront activées après la configuration du PIC
    
    Ok(())
}

/// Halte le CPU (pour toujours)
#[inline(always)]
pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Désactive les interruptions
#[inline(always)]
pub fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Active les interruptions
#[inline(always)]
pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Lit un byte depuis un port I/O
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

/// Écrit un byte vers un port I/O
#[inline(always)]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}
