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

macro_rules! interrupt_handler {
    ($name:ident, $handler_fn:path) => {
        #[unsafe(naked)]
        pub extern "C" fn $name() {
            core::arch::naked_asm!(
                // 1. Sauvegarder TOUS les registres (System V ABI)
                "push rax",
                "push rcx",
                "push rdx",
                "push rbx",
                "push rbp",
                "push rsi",
                "push rdi",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                
                // 2. Le CPU a déjà pushé 5*8=40 bytes
                // + nos 15*8=120 bytes = 160 total (multiple de 16 ✓)
                
                // 3. Passer le pointeur stack frame en premier argument (rdi)
                "mov rdi, rsp",
                "add rdi, 15*8",  // Pointer vers InterruptStackFrame
                
                // 4. Appeler le handler Rust
                "call {handler}",
                
                // 5. Restaurer les registres
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rdi",
                "pop rsi",
                "pop rbp",
                "pop rbx",
                "pop rdx",
                "pop rcx",
                "pop rax",
                
                // 6. Retour d'interruption
                "iretq",
                
                handler = sym $handler_fn
            )
        }
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
    
    // IMPORTANT: Envoyer EOI au PIC via le wrapper
    crate::arch::x86_64::pic_wrapper::send_eoi(32);  // IRQ 0 → Vector 32
}

/// Affiche le compteur de secondes sur la ligne 3
fn display_timer_count(seconds: u64) {
    let vga = 0xB8000 as *mut u16;
    unsafe {
        // Label
        let msg = b"[TIMER] Uptime: ";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(3 * 80 + i) = 0x0B00 | byte as u16; // Cyan
        }
        
        // Afficher les secondes (5 chiffres max)
        let mut temp = seconds;
        let mut digits = [b'0'; 5];
        for i in (0..5).rev() {
            digits[i] = b'0' + (temp % 10) as u8;
            temp /= 10;
        }
        
        for (i, &digit) in digits.iter().enumerate() {
            *vga.add(3 * 80 + 16 + i) = 0x0A00 | digit as u16; // Vert clair
        }
        
        // Ajouter " sec"
        let suffix = b" sec";
        for (i, &byte) in suffix.iter().enumerate() {
            *vga.add(3 * 80 + 21 + i) = 0x07000 | byte as u16;
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
    
    // Afficher sur la ligne 4 avec wrapping automatique
    static mut KEY_COUNT: usize = 0;
    let vga = 0xB8000 as *mut u16;
    
    unsafe {
        // Label permanent
        let msg = b"[KEYS] Last 20: ";
        for (i, &byte) in msg.iter().enumerate() {
            *vga.add(4 * 80 + i) = 0x0E00 | byte as u16; // Jaune
        }
        
        // Position pour les scancodes (20 scancodes max)
        let base_col = 16;
        let max_keys = 20;
        let pos = KEY_COUNT % max_keys;
        
        // Afficher le scancode en hexadécimal
        let high = (scancode >> 4) & 0x0F;
        let low = scancode & 0x0F;
        let high_char = if high < 10 { b'0' + high } else { b'A' + high - 10 };
        let low_char = if low < 10 { b'0' + low } else { b'A' + low - 10 };
        
        let col = base_col + (pos * 3);
        *vga.add(4 * 80 + col) = 0x0A00 | high_char as u16; // Vert
        *vga.add(4 * 80 + col + 1) = 0x0A00 | low_char as u16;
        *vga.add(4 * 80 + col + 2) = 0x0700 | b' ' as u16; // Espace
        
        KEY_COUNT += 1;
        
        // Effacer la prochaine position pour montrer le wrapping
        let next_pos = (pos + 1) % max_keys;
        let next_col = base_col + (next_pos * 3);
        *vga.add(4 * 80 + next_col) = 0x0800 | b'_' as u16; // Curseur gris
        *vga.add(4 * 80 + next_col + 1) = 0x0800 | b'_' as u16;
    }
    
    // EOI au PIC Master via le wrapper
    crate::arch::x86_64::pic_wrapper::send_eoi(33);  // IRQ 1 → Vector 33
}

// ============================================================================
// GÉNÉRATION DES WRAPPERS ASM
// ============================================================================

// Exceptions sans error code
interrupt_handler!(division_error_wrapper, division_error_handler);
interrupt_handler!(breakpoint_wrapper, breakpoint_handler);

// Exceptions avec error code (Double Fault, Page Fault)
#[unsafe(naked)]
pub extern "C" fn double_fault_wrapper() {
    core::arch::naked_asm!(
        // Le CPU a pushé l'error code AVANT le stack frame
        // Stack: [error_code] [SS] [RSP] [RFLAGS] [CS] [RIP]
        
        // Sauvegarder les registres
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        
        // Arguments: rdi=stack_frame, rsi=error_code
        "mov rdi, rsp",
        "add rdi, 15*8 + 8",      // Sauter registres + error_code
        "mov rsi, [rsp + 15*8]",   // Lire error_code
        
        // Appeler le handler
        "call double_fault_handler",
        
        // Restaurer (ne devrait jamais arriver ici)
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        
        "add rsp, 8",  // Pop error code
        "iretq"
    )
}

#[unsafe(naked)]
pub extern "C" fn page_fault_wrapper() {
    core::arch::naked_asm!(
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        
        "mov rdi, rsp",
        "add rdi, 15*8 + 8",
        "mov rsi, [rsp + 15*8]",
        
        "call page_fault_handler",
        
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        
        "add rsp, 8",
        "iretq"
    )
}

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
