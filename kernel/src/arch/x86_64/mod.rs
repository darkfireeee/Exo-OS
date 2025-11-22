//! Architecture x86_64 support for Exo-OS
//! 
//! Ce module gère les spécificités de l'architecture x86_64:
//! - GDT (Global Descriptor Table) pour la segmentation
//! - IDT (Interrupt Descriptor Table) pour les interruptions
//! - Gestion des interruptions et exceptions CPU
//! - Configuration des registres de contrôle

pub mod gdt;
pub mod idt;
pub mod interrupts;

// Constantes d'architecture
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_2MB: usize = 2 * 1024 * 1024;
pub const PAGE_SIZE_1GB: usize = 1024 * 1024 * 1024;

// Layout mémoire du kernel (identity mapped pour l'instant)
pub const KERNEL_PHYSICAL_BASE: usize = 0x0010_0000;    // 1MB (après GRUB)
pub const KERNEL_VIRTUAL_BASE: usize = 0x0010_0000;     // Identity mapped
pub const KERNEL_STACK_SIZE: usize = 16 * 1024;         // 16KB

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
    // On se contente d'initialiser l'IDT plus tard
    
    // TODO: Initialiser IDT
    // idt::init();
    
    // TODO: Activer les interruptions
    // interrupts::enable();
    
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
