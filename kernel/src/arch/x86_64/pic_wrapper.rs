//! Wrapper pour la crate pic8259
//! 
//! Utilise la crate pic8259 éprouvée pour gérer le PIC 8259
//! et éviter les problèmes de privilèges I/O.

use pic8259::ChainedPics;
use spin::Mutex;

/// Offset des IRQs dans l'IDT (32-47)
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;

/// Instance globale du PIC (protégée par un Mutex)
pub static PICS: Mutex<ChainedPics> = 
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Initialise le PIC en utilisant la crate pic8259
pub fn init_pic() {
    debug_msg(b"[PIC] Initializing using pic8259 crate...");
    
    unsafe {
        // La crate gère toute la séquence d'initialisation
        PICS.lock().initialize();
    }
    
    debug_msg(b"[PIC] Initialization complete");
    debug_msg(b"[PIC] IRQs mapped to vectors 32-47");
    
    // Par défaut, toutes les IRQs sont maskées
    // On unmask seulement le timer et le clavier
    unsafe {
        unmask_irq(0);  // Timer (IRQ 0 → Vector 32)
        unmask_irq(1);  // Keyboard (IRQ 1 → Vector 33)
    }
    
    debug_msg(b"[PIC] Enabled: Timer (IRQ 0), Keyboard (IRQ 1)");
}

/// Active une IRQ spécifique (0-15)
pub unsafe fn unmask_irq(irq: u8) {
    use x86_64::instructions::port::Port;
    
    if irq < 8 {
        // IRQ Master (0-7)
        let mut mask = Port::new(0x21);
        let current: u8 = mask.read();
        mask.write(current & !(1 << irq));
    } else {
        // IRQ Slave (8-15)
        let mut mask = Port::new(0xA1);
        let current: u8 = mask.read();
        mask.write(current & !(1 << (irq - 8)));
    }
}

/// Désactive une IRQ spécifique (0-15)
#[allow(dead_code)]
pub unsafe fn mask_irq(irq: u8) {
    use x86_64::instructions::port::Port;
    
    if irq < 8 {
        let mut mask = Port::new(0x21);
        let current: u8 = mask.read();
        mask.write(current | (1 << irq));
    } else {
        let mut mask = Port::new(0xA1);
        let current: u8 = mask.read();
        mask.write(current | (1 << (irq - 8)));
    }
}

/// Envoie End-Of-Interrupt au PIC
/// DOIT être appelé à la fin de chaque handler d'IRQ
pub fn send_eoi(irq: u8) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(irq);
    }
}

/// Désactive complètement les deux PICs
#[allow(dead_code)]
pub fn disable() {
    unsafe {
        PICS.lock().disable();
    }
    debug_msg(b"[PIC] Disabled");
}

// Fonction helper pour afficher des messages de debug
fn debug_msg(msg: &[u8]) {
    static mut ROW: usize = 6;
    let vga_buffer = 0xB8000 as *mut u16;
    
    unsafe {
        for (i, &byte) in msg.iter().enumerate() {
            let offset = ROW * 80 + i;
            if offset < 80 * 25 {
                vga_buffer.add(offset).write_volatile((byte as u16) | 0x0F00);
            }
        }
        ROW += 1;
        if ROW >= 25 {
            ROW = 6;
        }
    }
}
