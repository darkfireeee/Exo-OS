//! Handlers d'interruption corrects avec stack alignment 16 bytes
//! 
//! Résout le problème de boot loop causé par:
//! - Stack alignment incorrect (doit être 16-byte aligned avant call)
//! - Calling convention x86_64 System V ABI non respectée
//! - IRETQ qui nécessite un stack frame exact

use core::arch::asm;

/// Stack frame poussé par le CPU lors d'une interruption
#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

// ============================================================================
// MACRO POUR GÉNÉRER LES HANDLERS AVEC STACK ALIGNMENT CORRECT
// ============================================================================

// REMARQUE: Les handlers assembleur sont maintenant dans idt_handlers.asm
// pour éviter les problèmes LLVM avec naked_asm! sur Windows/MSVC
// Cette macro n'est plus utilisée
macro_rules! interrupt_handler {
    ($name:ident, $handler_fn:path) => {
        // Stub vide - les vrais handlers sont dans l'.asm
        pub extern "C" fn $name() {}
    };
}

// ============================================================================
// HANDLERS RUST (APPELÉS PAR LES WRAPPERS ASM)
// ============================================================================

/// Handler pour Division par Zéro (#DE)
#[no_mangle]
extern "C" fn division_error_handler(stack_frame: &InterruptStackFrame) {
    let vga = 0xB8000 as *mut u16;
    unsafe {
        let msg = b"[EXCEPTION] Division by zero!";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(24 * 80 + i) = 0x4F00 | byte as u16; // Blanc sur fond rouge
        }
    }
    loop { unsafe { asm!("hlt") } }
}

/// Handler pour Breakpoint (#BP)
#[no_mangle]
extern "C" fn breakpoint_handler(_stack_frame: &InterruptStackFrame) {
    // Afficher un message sur VGA pour confirmer que le gestionnaire fonctionne
    let vga = 0xB8000 as *mut u16;
    unsafe {
        let msg = b"[INT3] Breakpoint handled!";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(10 * 80 + i) = 0x0C00 | byte as u16;  // Rouge clair
        }
    }
    // Reprendre l'exécution normalement
}

/// Handler pour Double Fault (#DF) - CRITIQUE!
#[no_mangle]
extern "C" fn double_fault_handler(stack_frame: &InterruptStackFrame, error_code: u64) {
    let vga = 0xB8000 as *mut u16;
    unsafe {
        let msg = b"[DOUBLE FAULT] System halted!";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(24 * 80 + i) = 0x4F00 | byte as u16;
        }
    }
    loop { unsafe { asm!("cli; hlt") } }
}

/// Handler pour Page Fault (#PF)
#[no_mangle]
extern "C" fn page_fault_handler(stack_frame: &InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    
    let vga = 0xB8000 as *mut u16;
    unsafe {
        let msg = b"[PAGE FAULT] Access violation!";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(24 * 80 + i) = 0x4F00 | byte as u16;
        }
    }
    loop { unsafe { asm!("hlt") } }
}

/// Handler pour Timer (IRQ 0)
#[no_mangle]
extern "C" fn timer_interrupt_handler(_stack_frame: &InterruptStackFrame) {
    // Incrémenter les ticks
    crate::arch::x86_64::pit::tick();
    
    // Afficher le compteur toutes les 100 ticks (1 seconde à 100Hz)
    let ticks = crate::arch::x86_64::pit::get_ticks();
    if ticks % 100 == 0 {
        display_timer_count(ticks / 100);
    }
    
    // IMPORTANT: Envoyer EOI au PIC avant le scheduler
    crate::arch::x86_64::pic_wrapper::send_eoi(0);  // IRQ 0 (Timer)
    
    // Préemption: Appeler le scheduler tous les 10 ticks (10ms à 100Hz)
    if ticks % 10 == 0 {
        crate::scheduler::SCHEDULER.schedule();
    }
}

/// Affiche le compteur de secondes sur la ligne 16
fn display_timer_count(seconds: u64) {
    let vga = 0xB8000 as *mut u16;
    unsafe {
        // Label
        let msg = b"[UPTIME] ";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(16 * 80 + i) = 0x0B00 | byte as u16; // Cyan
        }
        
        // Afficher les secondes (5 chiffres max)
        let mut temp = seconds;
        let mut digits = [b'0'; 5];
        for i in (0..5).rev() {
            digits[i] = b'0' + (temp % 10) as u8;
            temp /= 10;
        }
        
        for (i, &digit) in digits.iter().enumerate() {
            *vga.add(16 * 80 + 9 + i) = 0x0A00 | digit as u16; // Vert clair
        }
        
        // Ajouter " seconds"
        let suffix = b" seconds";
        for (i, &byte) in suffix.iter().enumerate() {
            *vga.add(16 * 80 + 14 + i) = 0x0700 | byte as u16;
        }
    }
}

/// Handler pour Clavier (IRQ 1)
#[no_mangle]
extern "C" fn keyboard_interrupt_handler(_stack_frame: &InterruptStackFrame) {
    // Lire le scancode (obligatoire sinon le clavier se bloque)
    let scancode: u8;
    unsafe {
        asm!("in al, 0x60", out("al") scancode, options(nomem, nostack));
    }
    
    // Traiter le scancode avec le driver
    if let Some(c) = crate::drivers::input::hid::process_scancode(scancode) {
        // TODO: Send to shell when implemented
        display_typed_char(c);
    }
    
    // EOI au PIC Master via le wrapper
    crate::arch::x86_64::pic_wrapper::send_eoi(1);  // IRQ 1 (Keyboard)
}

/// Affiche les caractères tapés sur la ligne 17
fn display_typed_char(c: char) {
    static mut COL_POS: usize = 0;
    const MAX_COL: usize = 69; // Laisser de la place
    let vga = 0xB8000 as *mut u16;
    
    unsafe {
        // Label permanent
        let msg = b"[INPUT] ";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(17 * 80 + i) = 0x0E00 | byte as u16; // Jaune
        }
        
        let base_col = 8;
        
        // Gérer les caractères spéciaux
        match c {
            '\n' => {
                // Enter: nouvelle ligne (réinitialiser)
                COL_POS = 0;
                // Effacer la ligne
                for col in base_col..80 {
                    *vga.add(17 * 80 + col) = 0x0700 | b' ' as u16;
                }
            }
            '\x08' => {
                // Backspace: effacer le dernier caractère
                if COL_POS > 0 {
                    COL_POS -= 1;
                    *vga.add(17 * 80 + base_col + COL_POS) = 0x0700 | b' ' as u16;
                }
            }
            '\t' => {
                // Tab: 4 espaces
                for _ in 0..4 {
                    if COL_POS < MAX_COL {
                        *vga.add(17 * 80 + base_col + COL_POS) = 0x0A00 | b' ' as u16;
                        COL_POS += 1;
                    }
                }
            }
            c if c.is_ascii() => {
                // Caractère normal
                if COL_POS < MAX_COL {
                    *vga.add(17 * 80 + base_col + COL_POS) = 0x0A00 | c as u16;
                    COL_POS += 1;
                    
                    // Curseur clignotant à la position suivante
                    if COL_POS < MAX_COL {
                        *vga.add(17 * 80 + base_col + COL_POS) = 0x0F00 | b'_' as u16;
                    }
                }
            }
            _ => {} // Ignorer les caractères non-ASCII
        }
    }
}

// ============================================================================
// GÉNÉRATION DES WRAPPERS ASM
// ============================================================================

// Exceptions sans error code
interrupt_handler!(division_error_wrapper, division_error_handler);
interrupt_handler!(breakpoint_wrapper, breakpoint_handler);

// Exceptions avec error code (Double Fault, Page Fault)
// Stub temporaire - l'implémentation réelle est dans idt_handlers.asm
pub extern "C" fn double_fault_wrapper() {}

// Stub temporaire - l'implémentation réelle est dans idt_handlers.asm
pub extern "C" fn page_fault_wrapper() {}

// IRQs (pas d'error code)
interrupt_handler!(timer_wrapper, timer_interrupt_handler);
interrupt_handler!(keyboard_wrapper, keyboard_interrupt_handler);

// ============================================================================
// FONCTION PUBLIQUE POUR RÉCUPÉRER LES ADRESSES DES HANDLERS
// ============================================================================

pub struct HandlerAddresses {
    pub division_error: usize,
    pub breakpoint: usize,
    pub double_fault: usize,
    pub page_fault: usize,
    pub timer: usize,
    pub keyboard: usize,
}

pub fn get_handler_addresses() -> HandlerAddresses {
    HandlerAddresses {
        division_error: division_error_wrapper as usize,
        breakpoint: breakpoint_wrapper as usize,
        double_fault: double_fault_wrapper as usize,
        page_fault: page_fault_wrapper as usize,
        timer: timer_wrapper as usize,
        keyboard: keyboard_wrapper as usize,
    }
}
