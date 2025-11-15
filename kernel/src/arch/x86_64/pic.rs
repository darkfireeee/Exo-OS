// src/arch/x86_64/pic.rs
// Contrôleur d'interruptions 8259 (PIC) — remappage et gestion de masque

use x86_64::instructions::port::Port;

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const PIC_EOI: u8 = 0x20;

// ICW bits
const ICW1_INIT: u8 = 0x10;    // Initialization required
const ICW1_ICW4: u8 = 0x01;    // ICW4 present
const ICW4_8086: u8 = 0x01;    // 8086/88 mode

#[inline(always)]
fn io_wait() {
    // Classique: écrire sur le port 0x80 pour attendre ~1 µs
    unsafe { Port::<u8>::new(0x80).write(0); }
}

/// Remappe les deux PICs sur les offsets fournis (maître et esclave).
/// Retourne les anciens masques afin de pouvoir les restaurer si besoin.
pub fn remap(offset1: u8, offset2: u8) -> (u8, u8) {
    unsafe {
        let mut pic1_data = Port::<u8>::new(PIC1_DATA);
        let mut pic2_data = Port::<u8>::new(PIC2_DATA);
        let mut pic1_cmd = Port::<u8>::new(PIC1_CMD);
        let mut pic2_cmd = Port::<u8>::new(PIC2_CMD);

        let a1 = pic1_data.read();
        let a2 = pic2_data.read();

        // Start init sequence (cascade mode)
        pic1_cmd.write(ICW1_INIT | ICW1_ICW4);
        io_wait();
        pic2_cmd.write(ICW1_INIT | ICW1_ICW4);
        io_wait();

        // Set vector offsets
        pic1_data.write(offset1);
        io_wait();
        pic2_data.write(offset2);
        io_wait();

        // Tell master PIC that there is a slave PIC at IRQ2 (0000 0100)
        pic1_data.write(0x04);
        io_wait();
        // Tell slave PIC its cascade identity (2)
        pic2_data.write(0x02);
        io_wait();

        // Set PICs to 8086 mode
        pic1_data.write(ICW4_8086);
        io_wait();
        pic2_data.write(ICW4_8086);
        io_wait();

        // Restore saved masks
        pic1_data.write(a1);
        pic2_data.write(a2);

        (a1, a2)
    }
}

/// Initialise le PIC avec remappage et masque toutes les IRQ par défaut.
pub fn init(offset1: u8, offset2: u8) {
    let (_old1, _old2) = remap(offset1, offset2);
    // Masquer toutes les IRQ (on démasquera explicitement celles voulues)
    set_masks(0xFF, 0xFF);
}

/// Définit les masques des PICs (1 = masqué, 0 = autorisé)
pub fn set_masks(mask1: u8, mask2: u8) {
    unsafe {
        Port::<u8>::new(PIC1_DATA).write(mask1);
        Port::<u8>::new(PIC2_DATA).write(mask2);
    }
}

/// Démasque une IRQ donnée (0..15)
pub fn unmask_irq(irq: u8) {
    unsafe {
        if irq < 8 {
            let mut p = Port::<u8>::new(PIC1_DATA);
            let current: u8 = p.read();
            p.write(current & !(1 << irq));
        } else {
            let irq = irq - 8;
            let mut p = Port::<u8>::new(PIC2_DATA);
            let current: u8 = p.read();
            p.write(current & !(1 << irq));
        }
    }
}

/// Envoie un EOI (End Of Interrupt) au(x) PIC(s)
pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            Port::<u8>::new(PIC2_CMD).write(PIC_EOI);
        }
        Port::<u8>::new(PIC1_CMD).write(PIC_EOI);
    }
}
