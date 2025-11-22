// kernel/src/arch/x86_64/interrupts/pic.rs
//
// PROGRAMMABLE INTERRUPT CONTROLLER (8259 PIC)
// Configuration pour Exo-OS

use core::arch::asm;

/// Ports I/O du PIC Master (gère IRQ 0-7)
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;

/// Ports I/O du PIC Slave (gère IRQ 8-15)
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Commandes du PIC
const PIC_EOI: u8 = 0x20;          // End of Interrupt
const ICW1_INIT: u8 = 0x11;        // Initialization Command Word 1
const ICW4_8086: u8 = 0x01;        // ICW4: 8086 mode

/// Structure pour gérer le PIC
pub struct Pic {
    master_offset: u8,  // Offset dans l'IDT pour IRQ master (32 typiquement)
    slave_offset: u8,   // Offset dans l'IDT pour IRQ slave (40 typiquement)
}

impl Pic {
    /// Crée une nouvelle instance du PIC
    pub const fn new(master_offset: u8, slave_offset: u8) -> Self {
        Pic {
            master_offset,
            slave_offset,
        }
    }

    /// Initialise le PIC (remapping des IRQs)
    pub unsafe fn init(&self) {
        // Sauvegarder les masques actuels
        let master_mask = inb(PIC1_DATA);
        let slave_mask = inb(PIC2_DATA);

        // ========================================
        // INITIALISATION CASCADE
        // ========================================

        // ICW1: Démarre l'initialisation en cascade
        outb(PIC1_COMMAND, ICW1_INIT);
        io_wait();
        outb(PIC2_COMMAND, ICW1_INIT);
        io_wait();

        // ICW2: Vecteurs d'interruption (offset dans l'IDT)
        outb(PIC1_DATA, self.master_offset);  // Master commence à 32
        io_wait();
        outb(PIC2_DATA, self.slave_offset);   // Slave commence à 40
        io_wait();

        // ICW3: Configuration cascade
        // Master: IRQ2 est connecté au slave
        // Slave: Son ID cascade est 2
        outb(PIC1_DATA, 0x04);  // 0000 0100 = IRQ2
        io_wait();
        outb(PIC2_DATA, 0x02);  // 0000 0010 = Cascade ID 2
        io_wait();

        // ICW4: Mode 8086
        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();

        // Restaurer les masques (tous désactivés au début)
        outb(PIC1_DATA, 0xFF);  // Masquer toutes les IRQs master
        io_wait();
        outb(PIC2_DATA, 0xFF);  // Masquer toutes les IRQs slave
        io_wait();
    }

    /// Active une IRQ spécifique (0-15)
    pub unsafe fn unmask_irq(&self, irq: u8) {
        let port = if irq < 8 {
            PIC1_DATA
        } else {
            PIC2_DATA
        };

        let irq_bit = irq % 8;
        let current_mask = inb(port);
        let new_mask = current_mask & !(1 << irq_bit);
        
        outb(port, new_mask);
    }

    /// Désactive une IRQ spécifique (0-15)
    pub unsafe fn mask_irq(&self, irq: u8) {
        let port = if irq < 8 {
            PIC1_DATA
        } else {
            PIC2_DATA
        };

        let irq_bit = irq % 8;
        let current_mask = inb(port);
        let new_mask = current_mask | (1 << irq_bit);
        
        outb(port, new_mask);
    }

    /// Envoie un End of Interrupt (EOI) au PIC
    pub unsafe fn send_eoi(&self, irq: u8) {
        // Si l'IRQ vient du slave, envoyer EOI aux deux
        if irq >= 8 {
            outb(PIC2_COMMAND, PIC_EOI);
        }
        // Toujours envoyer EOI au master
        outb(PIC1_COMMAND, PIC_EOI);
    }

    /// Désactive complètement les deux PICs
    pub unsafe fn disable(&self) {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

// ============================================================================
// FONCTIONS I/O BAS NIVEAU
// ============================================================================

/// Écrit un byte sur un port I/O
#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Lit un byte depuis un port I/O
#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Attente I/O (nécessaire pour les vieux hardware)
#[inline(always)]
unsafe fn io_wait() {
    // Port 0x80 est un port "poubelle" utilisé pour les delays
    outb(0x80, 0);
}

// ============================================================================
// INSTANCE GLOBALE
// ============================================================================

static PIC: Pic = Pic::new(32, 40);  // IRQs mappées à 32-47

/// Initialise le PIC (à appeler depuis kernel_main)
pub fn init_pic() {
    unsafe {
        PIC.init();
        
        // Activer seulement Timer (IRQ 0) et Clavier (IRQ 1) au début
        PIC.unmask_irq(0);  // Timer
        PIC.unmask_irq(1);  // Keyboard
        
        // Le reste des IRQs reste masqué
    }
    
    serial_println!("[PIC] Initialized (IRQs 32-47)");
    serial_println!("[PIC] Enabled: Timer (IRQ 0), Keyboard (IRQ 1)");
}

/// Envoie EOI pour une IRQ donnée (à appeler dans les handlers)
pub fn send_eoi(irq: u8) {
    unsafe {
        PIC.send_eoi(irq);
    }
}

/// Désactive le PIC (si vous passez à l'APIC)
pub fn disable_pic() {
    unsafe {
        PIC.disable();
    }
    serial_println!("[PIC] Disabled");
}

// ============================================================================
// HELPER POUR SERIAL OUTPUT
// ============================================================================

macro_rules! serial_println {
    ($($arg:tt)*) => {
        // TODO: Implémenter selon votre serial driver
    };
}
