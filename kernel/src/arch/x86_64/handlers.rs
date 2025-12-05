//! Handlers d'interruption corrects avec stack alignment 16 bytes
//! 
//! Résout le problème de boot loop causé par:
//! - Stack alignment incorrect (doit être 16-byte aligned avant call)
//! - Calling convention x86_64 System V ABI non respectée
//! - IRETQ qui nécessite un stack frame exact

use core::arch::{asm, global_asm};

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
// WRAPPERS ASSEMBLEUR POUR LES HANDLERS D'INTERRUPTION
// ============================================================================

global_asm!(
    ".intel_syntax noprefix",
    "",
    "# Common interrupt wrapper macro",
    "# Saves all registers, aligns stack, calls handler, restores, iretq",
    "",
    ".global timer_wrapper",
    "timer_wrapper:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",        // rdi = pointer to saved regs (for stack frame)
    "    add rdi, 72",         // Skip over saved regs to get to interrupt frame
    "    call timer_interrupt_handler",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
    "",
    ".global keyboard_wrapper",
    "keyboard_wrapper:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",
    "    add rdi, 72",
    "    call keyboard_interrupt_handler",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
    "",
    ".global division_error_wrapper",
    "division_error_wrapper:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",
    "    add rdi, 72",
    "    call division_error_handler",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
    "",
    ".global breakpoint_wrapper",
    "breakpoint_wrapper:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",
    "    add rdi, 72",
    "    call breakpoint_handler",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
    "",
    ".global double_fault_wrapper",
    "double_fault_wrapper:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",
    "    add rdi, 72",
    "    call double_fault_handler",
    "    # Double fault doesn't return",
    "1:  hlt",
    "    jmp 1b",
    "",
    ".global page_fault_wrapper",
    "page_fault_wrapper:",
    "    # Page fault has error code on stack - skip it for now",
    "    add rsp, 8",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    mov rdi, rsp",
    "    add rdi, 72",
    "    call page_fault_handler",
    "    # Page fault handler loops, doesn't return",
    "1:  hlt",
    "    jmp 1b",
    "",
    ".att_syntax prefix",
);

// External wrapper functions defined in global_asm! above
extern "C" {
    pub fn timer_wrapper();
    pub fn keyboard_wrapper();
    pub fn division_error_wrapper();
    pub fn breakpoint_wrapper();
    pub fn double_fault_wrapper();
    pub fn page_fault_wrapper();
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
/// Intégré avec COW (Copy-On-Write) pour Phase 0
#[no_mangle]
extern "C" fn page_fault_handler(_stack_frame: &InterruptStackFrame, error_code: u64) {
    use crate::memory::address::VirtualAddress;
    use crate::logger;
    
    // Lire CR2 (adresse qui a causé le fault)
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    
    let fault_addr = VirtualAddress::new(cr2 as usize);
    
    // Décoder error_code
    let is_present = (error_code & 0x1) != 0;
    let is_write = (error_code & 0x2) != 0;
    let is_user = (error_code & 0x4) != 0;
    let is_reserved = (error_code & 0x8) != 0;
    let is_instruction = (error_code & 0x10) != 0;
    
    // Log détaillé (uniquement en debug pour éviter spam)
    #[cfg(debug_assertions)]
    logger::debug(&alloc::format!(
        "[PAGE FAULT] addr={:?} present={} write={} user={} reserved={} instr={}",
        fault_addr, is_present, is_write, is_user, is_reserved, is_instruction
    ));
    
    // Appeler le handler de mémoire virtuelle
    match crate::memory::virtual_mem::handle_page_fault(fault_addr, error_code) {
        Ok(()) => {
            // Fault géré avec succès (COW, demand paging, etc.)
            #[cfg(debug_assertions)]
            logger::debug(&alloc::format!("[PAGE FAULT] Successfully handled at {:?}", fault_addr));
            return;
        }
        Err(e) => {
            // Fault non récupérable - afficher erreur et panic
            logger::error("╔══════════════════════════════════════════════════════════╗");
            logger::error("║              FATAL PAGE FAULT                            ║");
            logger::error("╚══════════════════════════════════════════════════════════╝");
            logger::error(&alloc::format!("  Address:     {:?}", fault_addr));
            logger::error(&alloc::format!("  Error code:  0x{:x}", error_code));
            logger::error(&alloc::format!("  Present:     {}", is_present));
            logger::error(&alloc::format!("  Write:       {}", is_write));
            logger::error(&alloc::format!("  User:        {}", is_user));
            logger::error(&alloc::format!("  Reserved:    {}", is_reserved));
            logger::error(&alloc::format!("  Instruction: {}", is_instruction));
            logger::error(&alloc::format!("  Error:       {:?}", e));
            
            // VGA pour visibilité immédiate
            let vga = 0xB8000 as *mut u16;
            unsafe {
                let msg = b"[FATAL PAGE FAULT] See serial log";
                for (i, &byte) in msg.iter().enumerate() {
                    *vga.add(24 * 80 + i) = 0x4F00 | byte as u16;
                }
            }
            
            panic!("Unrecoverable page fault at {:?}: {:?}", fault_addr, e);
        }
    }
}

/// Handler pour Timer (IRQ 0)
#[no_mangle]
extern "C" fn timer_interrupt_handler(_stack_frame: &InterruptStackFrame) {
    // Debug très tôt: écrire sur VGA pour confirmer entrée dans handler
    let vga = 0xB8000 as *mut u16;
    unsafe {
        // Écrire le numéro de tick sur ligne 0, colonnes 70-79
        static mut CALL_COUNT: u64 = 0;
        CALL_COUNT += 1;
        
        // Écrire 'H' pour "Handler" au coin supérieur droit
        let digit = (CALL_COUNT % 10) as u8;
        *vga.add(78) = 0x0E00 | b'0'.wrapping_add(digit) as u16;  // Jaune
        *vga.add(79) = 0x0A00 | b'H' as u16;  // Vert
    }
    
    // Incrémenter les ticks
    crate::arch::x86_64::pit::tick();
    
    // Afficher le compteur toutes les 100 ticks (1 seconde à 100Hz)
    let ticks = crate::arch::x86_64::pit::get_ticks();
    if ticks % 100 == 0 {
        display_timer_count(ticks / 100);
        crate::logger::early_print("[T]"); // Timer still running
    }
    
    // IMPORTANT: Envoyer EOI au PIC 8259
    crate::arch::x86_64::pic_wrapper::send_eoi(0);  // IRQ 0 = Timer
    
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
    if let Some(c) = crate::drivers::input::keyboard::process_scancode(scancode) {
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
