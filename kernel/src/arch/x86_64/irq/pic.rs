//! Legacy 8259A PIC handling.
//!
//! Exo-OS routes hardware interrupts through the I/O APIC/LAPIC path. The
//! firmware 8259A PIC can still be left enabled by BIOS/GRUB, especially the
//! PIT on IRQ0. If that IRQ is delivered before the PIC is remapped, it arrives
//! as vector 0x08, which collides with the CPU double-fault exception in long
//! mode. We remap it to the normal IRQ window, then mask every legacy line.

use core::sync::atomic::{AtomicBool, Ordering};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

const MASTER_OFFSET: u8 = crate::arch::x86_64::idt::IRQ_BASE;
const SLAVE_OFFSET: u8 = crate::arch::x86_64::idt::IRQ_BASE + 8;

static PIC_MASKED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
unsafe fn io_wait() {
    // SAFETY: port 0x80 is the traditional POST/debug delay port.
    unsafe {
        crate::arch::x86_64::outb(0x80, 0);
    }
}

#[inline(always)]
unsafe fn outb_wait(port: u16, value: u8) {
    // SAFETY: caller is performing the standard 8259A initialization sequence.
    unsafe {
        crate::arch::x86_64::outb(port, value);
        io_wait();
    }
}

/// Remaps the legacy PIC away from exception vectors and masks all IRQ lines.
///
/// This is idempotent and safe to call from early boot before interrupts are
/// enabled. It deliberately does not restore firmware masks: once LAPIC/IOAPIC
/// owns interrupt delivery, unmasking the PIC would reintroduce duplicate and
/// wrongly vectored interrupts.
pub fn remap_and_mask() {
    if PIC_MASKED.swap(true, Ordering::AcqRel) {
        return;
    }

    // SAFETY: these are the architectural 8259A command/data ports. The sequence
    // is the standard ICW1..4 initialization followed by a full mask.
    unsafe {
        crate::arch::x86_64::outb(PIC1_DATA, 0xFF);
        crate::arch::x86_64::outb(PIC2_DATA, 0xFF);
        io_wait();

        outb_wait(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        outb_wait(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);

        outb_wait(PIC1_DATA, MASTER_OFFSET);
        outb_wait(PIC2_DATA, SLAVE_OFFSET);

        outb_wait(PIC1_DATA, 1 << 2);
        outb_wait(PIC2_DATA, 2);

        outb_wait(PIC1_DATA, ICW4_8086);
        outb_wait(PIC2_DATA, ICW4_8086);

        crate::arch::x86_64::outb(PIC1_DATA, 0xFF);
        crate::arch::x86_64::outb(PIC2_DATA, 0xFF);
        io_wait();
    }
}

#[inline]
pub fn is_masked() -> bool {
    PIC_MASKED.load(Ordering::Acquire)
}
