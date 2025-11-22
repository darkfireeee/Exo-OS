// kernel/src/arch/x86_64/interrupts/idt.rs
//
// CONFIGURATION COMPLÈTE DE L'IDT POUR EXO-OS

use core::mem::size_of;
use super::handlers_safe::{get_handler_addresses, HandlerAddresses};

/// Entry dans l'IDT (16 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,      // Offset bits 0-15
    selector: u16,        // Code segment selector
    ist: u8,              // Interrupt Stack Table offset (0 = pas utilisé)
    flags: u8,            // Type et attributs
    offset_mid: u16,      // Offset bits 16-31
    offset_high: u32,     // Offset bits 32-63
    reserved: u32,        // Doit être 0
}

impl IdtEntry {
    /// Crée une entry vide
    pub const fn missing() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Crée une entry pour un handler
    pub fn new(handler: usize, selector: u16, ist: u8, dpl: u8) -> Self {
        let offset = handler as u64;
        
        // Flags: Present=1, DPL=dpl, Type=0xE (Interrupt Gate)
        // Type 0xE = 64-bit Interrupt Gate
        let flags = 0x80 | ((dpl & 0x3) << 5) | 0x0E;
        
        IdtEntry {
            offset_low: (offset & 0xFFFF) as u16,
            selector,
            ist,
            flags,
            offset_mid: ((offset >> 16) & 0xFFFF) as u16,
            offset_high: ((offset >> 32) & 0xFFFFFFFF) as u32,
            reserved: 0,
        }
    }
}

/// Descripteur de l'IDT (10 bytes)
#[repr(C, packed)]
pub struct IdtDescriptor {
    limit: u16,
    base: u64,
}

/// Interrupt Descriptor Table (256 entrées)
#[repr(C, align(16))]
pub struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    /// Crée une IDT vide
    pub const fn new() -> Self {
        Idt {
            entries: [IdtEntry::missing(); 256],
        }
    }

    /// Configure les handlers essentiels
    pub fn setup_handlers(&mut self, code_selector: u16) {
        let handlers = get_handler_addresses();
        
        // ========================================
        // EXCEPTIONS (0-31)
        // ========================================
        
        // #DE (0) - Division Error
        self.entries[0] = IdtEntry::new(handlers.division_error, code_selector, 0, 0);
        
        // #DB (1) - Debug
        // TODO: Implémenter
        
        // #BP (3) - Breakpoint (DPL=3 pour int3 en userspace)
        self.entries[3] = IdtEntry::new(handlers.breakpoint, code_selector, 0, 3);
        
        // #OF (4) - Overflow
        // TODO: Implémenter
        
        // #BR (5) - Bound Range Exceeded
        // TODO: Implémenter
        
        // #UD (6) - Invalid Opcode
        // TODO: Implémenter
        
        // #NM (7) - Device Not Available
        // TODO: Implémenter
        
        // #DF (8) - Double Fault (CRITIQUE!)
        self.entries[8] = IdtEntry::new(handlers.double_fault, code_selector, 1, 0);
        // IST=1 pour utiliser une stack séparée (évite stack overflow récursif)
        
        // #TS (10) - Invalid TSS
        // TODO: Implémenter
        
        // #NP (11) - Segment Not Present
        // TODO: Implémenter
        
        // #SS (12) - Stack Segment Fault
        // TODO: Implémenter
        
        // #GP (13) - General Protection Fault
        // TODO: Implémenter
        
        // #PF (14) - Page Fault
        self.entries[14] = IdtEntry::new(handlers.page_fault, code_selector, 0, 0);
        
        // #MF (16) - x87 Floating Point Exception
        // TODO: Implémenter
        
        // #AC (17) - Alignment Check
        // TODO: Implémenter
        
        // #MC (18) - Machine Check
        // TODO: Implémenter
        
        // #XM (19) - SIMD Floating Point Exception
        // TODO: Implémenter
        
        // #VE (20) - Virtualization Exception
        // TODO: Implémenter
        
        // ========================================
        // IRQs (32-47) - 8259 PIC
        // ========================================
        
        // IRQ 0 (32) - PIT Timer
        self.entries[32] = IdtEntry::new(handlers.timer, code_selector, 0, 0);
        
        // IRQ 1 (33) - Keyboard
        self.entries[33] = IdtEntry::new(handlers.keyboard, code_selector, 0, 0);
        
        // IRQ 2-15: TODO selon vos besoins
        
        // ========================================
        // SYSCALLS (128 typiquement)
        // ========================================
        
        // TODO: Handler syscall (DPL=3)
        // self.entries[128] = IdtEntry::new(syscall_handler, code_selector, 0, 3);
    }

    /// Charge l'IDT dans le CPU
    pub fn load(&self) {
        let descriptor = IdtDescriptor {
            limit: (size_of::<Idt>() - 1) as u16,
            base: self as *const _ as u64,
        };

        unsafe {
            core::arch::asm!(
                "lidt [{}]",
                in(reg) &descriptor,
                options(nostack, preserves_flags)
            );
        }
    }
}

// ============================================================================
// INSTANCE GLOBALE DE L'IDT
// ============================================================================

static mut IDT: Idt = Idt::new();

/// Initialise l'IDT (à appeler depuis kernel_main)
pub fn init_idt() {
    unsafe {
        // Code segment selector (0x08 pour GDT entry 1)
        let code_selector = 0x08;
        
        // Setup des handlers
        IDT.setup_handlers(code_selector);
        
        // Chargement dans le CPU
        IDT.load();
    }
    
    serial_println!("[IDT] Initialized with handlers");
}

/// Test de l'IDT (division par zéro)
pub fn test_idt_division_by_zero() {
    serial_println!("[IDT TEST] Triggering division by zero...");
    unsafe {
        core::arch::asm!(
            "mov rax, 0",
            "mov rbx, 0",
            "div rbx",  // Division par zéro
            options(noreturn)
        );
    }
}

/// Test de l'IDT (breakpoint)
pub fn test_idt_breakpoint() {
    serial_println!("[IDT TEST] Triggering breakpoint...");
    unsafe {
        core::arch::asm!("int3");
    }
    serial_println!("[IDT TEST] Breakpoint returned successfully!");
}

// ============================================================================
// HELPER POUR SERIAL OUTPUT
// ============================================================================

macro_rules! serial_println {
    ($($arg:tt)*) => {
        // TODO: Implémenter selon votre serial driver
    };
}
