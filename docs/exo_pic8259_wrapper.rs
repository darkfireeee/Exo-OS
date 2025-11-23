// kernel/src/arch/x86_64/interrupts/pic_wrapper.rs
//
// WRAPPER POUR LA CRATE PIC8259 (SOLUTION STABLE)

use pic8259::ChainedPics;
use spin::Mutex;

/// Offset des IRQs dans l'IDT
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;

/// Instance globale du PIC (protégée par un Mutex)
pub static PICS: Mutex<ChainedPics> = 
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Initialise le PIC en utilisant la crate pic8259
pub fn init_pic() {
    println!("[PIC] Initializing using pic8259 crate...");
    
    unsafe {
        // La crate gère toute la séquence d'initialisation
        PICS.lock().initialize();
    }
    
    println!("[PIC] Initialization complete");
    println!("[PIC] IRQs mapped to vectors 32-47");
    
    // Par défaut, toutes les IRQs sont maskées
    // On unmask seulement le timer et le clavier
    unsafe {
        unmask_irq(0);  // Timer (IRQ 0 → Vector 32)
        unmask_irq(1);  // Keyboard (IRQ 1 → Vector 33)
    }
    
    println!("[PIC] Enabled: Timer (IRQ 0), Keyboard (IRQ 1)");
}

/// Active une IRQ spécifique (0-15)
pub unsafe fn unmask_irq(irq: u8) {
    let mut pics = PICS.lock();
    
    if irq < 8 {
        // IRQ Master (0-7)
        let mut mask = x86_64::instructions::port::Port::new(0x21);
        let current: u8 = mask.read();
        mask.write(current & !(1 << irq));
    } else {
        // IRQ Slave (8-15)
        let mut mask = x86_64::instructions::port::Port::new(0xA1);
        let current: u8 = mask.read();
        mask.write(current & !(1 << (irq - 8)));
    }
}

/// Désactive une IRQ spécifique (0-15)
pub unsafe fn mask_irq(irq: u8) {
    if irq < 8 {
        let mut mask = x86_64::instructions::port::Port::new(0x21);
        let current: u8 = mask.read();
        mask.write(current | (1 << irq));
    } else {
        let mut mask = x86_64::instructions::port::Port::new(0xA1);
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
pub fn disable() {
    unsafe {
        let mut pics = PICS.lock();
        pics.disable();
    }
    println!("[PIC] Disabled");
}

// Placeholder pour println
macro_rules! println {
    ($($arg:tt)*) => {
        // TODO: Utiliser votre implementation VGA/Serial
    };
}
