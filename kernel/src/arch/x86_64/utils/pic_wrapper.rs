//! PIC 8259 driver - Manual implementation
//! 
//! Direct programming of the 8259 PIC without external crate

use spin::Mutex;

/// MSR for APIC base
const IA32_APIC_BASE: u32 = 0x1B;

/// I/O APIC address
const IOAPIC_BASE: usize = 0xFEC00000;

/// Mask all I/O APIC interrupts to ensure PIC mode works
pub fn disable_ioapic() {
    crate::logger::early_print("[PIC] Masking all I/O APIC entries...\n");
    
    unsafe {
        let regsel = IOAPIC_BASE as *mut u32;
        let win = (IOAPIC_BASE + 0x10) as *mut u32;
        
        // Read version to get number of entries
        core::ptr::write_volatile(regsel, 0x01);  // IOAPIC_VER
        let ver = core::ptr::read_volatile(win);
        let max_entries = ((ver >> 16) & 0xFF) + 1;
        
        crate::logger::early_print(&alloc::format!("[PIC] I/O APIC has {} entries\n", max_entries));
        
        // Mask each redirection entry (set bit 16 of low dword)
        for i in 0..max_entries {
            let redtbl_low = 0x10 + (i * 2) as u8;
            
            // Read current value
            core::ptr::write_volatile(regsel, redtbl_low as u32);
            let current = core::ptr::read_volatile(win);
            
            // Set mask bit (bit 16)
            core::ptr::write_volatile(regsel, redtbl_low as u32);
            core::ptr::write_volatile(win, current | (1 << 16));
        }
    }
    
    crate::logger::early_print("[PIC] ✓ I/O APIC fully masked\n");
}

/// Disable the Local APIC to force legacy PIC mode
/// This must be called BEFORE initializing the PIC
pub fn disable_apic() {
    crate::logger::early_print("[PIC] Disabling Local APIC to force PIC mode...\n");
    
    unsafe {
        // Read current APIC base MSR
        let low: u32;
        let high: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
        
        let apic_base = ((high as u64) << 32) | (low as u64);
        crate::logger::early_print(&alloc::format!("[PIC] APIC_BASE MSR = 0x{:016X}\n", apic_base));
        
        // Clear bit 11 (global enable bit) to disable the APIC
        let new_base = apic_base & !(1 << 11);
        let new_low = new_base as u32;
        let new_high = (new_base >> 32) as u32;
        
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_APIC_BASE,
            in("eax") new_low,
            in("edx") new_high,
            options(nomem, nostack, preserves_flags)
        );
        
        crate::logger::early_print("[PIC] ✓ Local APIC disabled\n");
    }
}

/// PIC ports
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// ICW1 flags
const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;

/// ICW4 flags
const ICW4_8086: u8 = 0x01;

/// Offset des IRQs dans l'IDT (32-47)
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;

/// Helper functions for port I/O
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    val
}

/// Small delay for PIC
unsafe fn io_wait() {
    outb(0x80, 0);
}

/// Initialise le PIC avec les offsets corrects
pub fn init_pic() {
    crate::logger::early_print("[PIC] Manual initialization starting...\n");
    
    unsafe {
        // Save masks (will be restored as 0xFF initially)
        let _mask1 = inb(PIC1_DATA);
        let _mask2 = inb(PIC2_DATA);
        
        crate::logger::early_print("[PIC] Sending ICW1 (init + need ICW4)...\n");
        
        // Start initialization sequence (ICW1)
        outb(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        
        crate::logger::early_print("[PIC] Sending ICW2 (vector offsets 32, 40)...\n");
        
        // ICW2: Set vector offsets
        outb(PIC1_DATA, PIC_1_OFFSET);  // Master: vectors 32-39
        io_wait();
        outb(PIC2_DATA, PIC_2_OFFSET);  // Slave: vectors 40-47
        io_wait();
        
        crate::logger::early_print("[PIC] Sending ICW3 (cascade on IRQ2)...\n");
        
        // ICW3: Tell Master that Slave is at IRQ2
        outb(PIC1_DATA, 4);  // Slave at IRQ2 (bit 2 = 1)
        io_wait();
        outb(PIC2_DATA, 2);  // Slave cascade identity = 2
        io_wait();
        
        crate::logger::early_print("[PIC] Sending ICW4 (8086 mode)...\n");
        
        // ICW4: 8086 mode
        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();
        
        crate::logger::early_print("[PIC] Masking all IRQs...\n");
        
        // Mask all initially
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
    
    crate::logger::early_print("[PIC] Base initialization complete\n");
    
    // Unmask Timer (IRQ0) and Keyboard (IRQ1)
    unsafe {
        let mut mask = inb(PIC1_DATA);
        crate::logger::early_print(&alloc::format!("[PIC] Read mask before unmask: 0x{:02X}\n", mask));
        
        mask &= !(1 << 0);  // Unmask IRQ0 (timer)
        mask &= !(1 << 1);  // Unmask IRQ1 (keyboard)
        outb(PIC1_DATA, mask);
        
        let verify = inb(PIC1_DATA);
        crate::logger::early_print(&alloc::format!("[PIC] Wrote 0x{:02X}, verify read: 0x{:02X}\n", mask, verify));
    }
    
    crate::logger::early_print("[PIC] Timer and Keyboard unmasked\n");
}

/// Envoie End-Of-Interrupt au PIC
pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, 0x20);  // EOI to slave
        }
        outb(PIC1_COMMAND, 0x20);  // EOI to master
    }
}

// For compatibility with old code that uses PICS
use pic8259::ChainedPics;
pub static PICS: Mutex<ChainedPics> = 
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Désactive complètement les deux PICs
#[allow(dead_code)]
pub fn disable() {
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}
