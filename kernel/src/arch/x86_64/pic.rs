//! PIC (Programmable Interrupt Controller) 8259
//! 
//! Gère le remapping et la configuration du PIC pour les interruptions matérielles

use crate::arch::x86_64::{inb, outb};

/// Ports du PIC master
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;

/// Ports du PIC slave
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Commandes PIC
const PIC_EOI: u8 = 0x20; // End of Interrupt

/// Commandes d'initialisation
const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

/// Offset des IRQ après remapping
pub const PIC1_OFFSET: u8 = 32; // IRQ 0-7 → INT 32-39
pub const PIC2_OFFSET: u8 = 40; // IRQ 8-15 → INT 40-47

/// Initialise et remappe le PIC
/// 
/// Par défaut, le PIC utilise les vecteurs 0-15 qui sont réservés pour les exceptions CPU.
/// On les remappe vers 32-47 pour éviter les conflits.
pub fn init() {
    unsafe {
        // Sauvegarder les masques actuels
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        // Commencer l'initialisation (ICW1)
        outb(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();

        // Définir les offsets (ICW2)
        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();
        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        // Configurer le chaînage (ICW3)
        outb(PIC1_DATA, 4); // IRQ2 est connecté au slave
        io_wait();
        outb(PIC2_DATA, 2); // Cascade identity
        io_wait();

        // Mode 8086 (ICW4)
        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();

        // Restaurer les masques
        outb(PIC1_DATA, mask1);
        outb(PIC2_DATA, mask2);
    }
}

/// Désactive toutes les interruptions du PIC
pub fn disable_all() {
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

/// Active une IRQ spécifique
pub fn unmask_irq(irq: u8) {
    unsafe {
        let port = if irq < 8 {
            PIC1_DATA
        } else {
            PIC2_DATA
        };
        
        let value = inb(port) & !(1 << (irq % 8));
        outb(port, value);
    }
}

/// Désactive une IRQ spécifique
pub fn mask_irq(irq: u8) {
    unsafe {
        let port = if irq < 8 {
            PIC1_DATA
        } else {
            PIC2_DATA
        };
        
        let value = inb(port) | (1 << (irq % 8));
        outb(port, value);
    }
}

/// Envoie un EOI (End of Interrupt) au PIC
pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, PIC_EOI);
        }
        outb(PIC1_COMMAND, PIC_EOI);
    }
}

/// Attend un cycle I/O (pour les vieux matériels)
unsafe fn io_wait() {
    outb(0x80, 0);
}

/// Active les interruptions CPU
pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Désactive les interruptions CPU
pub fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
}
