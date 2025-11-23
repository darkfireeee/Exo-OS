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
        use core::arch::asm;
        
        // ICW1: Commencer l'initialisation
        asm!("out dx, al", in("dx") 0x20u16, in("al") 0x11u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait
        asm!("out dx, al", in("dx") 0xA0u16, in("al") 0x11u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait

        // ICW2: Définir les offsets (32 pour master, 40 pour slave)
        asm!("out dx, al", in("dx") 0x21u16, in("al") 32u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait
        asm!("out dx, al", in("dx") 0xA1u16, in("al") 40u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait

        // ICW3: Configurer le chaînage
        asm!("out dx, al", in("dx") 0x21u16, in("al") 4u8, options(nomem, nostack)); // IRQ2 = slave
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait
        asm!("out dx, al", in("dx") 0xA1u16, in("al") 2u8, options(nomem, nostack)); // Cascade identity
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait

        // ICW4: Mode 8086
        asm!("out dx, al", in("dx") 0x21u16, in("al") 0x01u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait
        asm!("out dx, al", in("dx") 0xA1u16, in("al") 0x01u8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0x80u16, in("al") 0u8, options(nomem, nostack)); // io_wait

        // Masquer toutes les IRQ par défaut
        asm!("out dx, al", in("dx") 0x21u16, in("al") 0xFFu8, options(nomem, nostack));
        asm!("out dx, al", in("dx") 0xA1u16, in("al") 0xFFu8, options(nomem, nostack));
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
