// kernel/src/arch/x86_64/interrupts/handlers_safe.rs
//
// HANDLERS D'INTERRUPTION CORRECTS POUR EXO-OS
// Résout les problèmes de boot loop avec naked functions

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

/// Registres sauvegardés par nos handlers
#[repr(C)]
pub struct SavedRegisters {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
}

// ============================================================================
// MACRO POUR GÉNÉRER LES HANDLERS PROPREMENT
// ============================================================================

macro_rules! interrupt_handler {
    ($name:ident, $handler_fn:path) => {
        #[naked]
        pub unsafe extern "C" fn $name() {
            unsafe {
                asm!(
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
                    
                    // 2. Aligner la stack sur 16 bytes (CRITIQUE!)
                    // Le CPU a pushé 5*8=40 bytes (pas multiple de 16)
                    // + nos 15*8=120 bytes = 160 total (multiple de 16, OK!)
                    
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
                    
                    handler = sym $handler_fn,
                    options(noreturn)
                )
            }
        }
    };
}

// ============================================================================
// HANDLERS RUST (APPELÉS PAR LES WRAPPERS ASM)
// ============================================================================

/// Handler pour les exceptions sans error code
#[no_mangle]
extern "C" fn generic_exception_handler(stack_frame: &InterruptStackFrame) {
    serial_println!("[EXCEPTION] RIP: {:#x}", stack_frame.instruction_pointer);
    // NE PAS PANIC ici, juste logger
}

/// Handler pour Division par Zéro (#DE)
#[no_mangle]
extern "C" fn division_error_handler(stack_frame: &InterruptStackFrame) {
    serial_println!("[#DE] Division by zero at RIP: {:#x}", stack_frame.instruction_pointer);
    // Vous pouvez kill le process ici
    loop { unsafe { asm!("hlt") } }
}

/// Handler pour Breakpoint (#BP)
#[no_mangle]
extern "C" fn breakpoint_handler(stack_frame: &InterruptStackFrame) {
    serial_println!("[#BP] Breakpoint at RIP: {:#x}", stack_frame.instruction_pointer);
    // Reprendre l'exécution normalement
}

/// Handler pour Double Fault (#DF) - CRITIQUE!
#[no_mangle]
extern "C" fn double_fault_handler(stack_frame: &InterruptStackFrame, error_code: u64) {
    serial_println!("[#DF] DOUBLE FAULT!");
    serial_println!("  RIP: {:#x}", stack_frame.instruction_pointer);
    serial_println!("  Error Code: {:#x}", error_code);
    
    // Double Fault = game over, on halt
    loop { unsafe { asm!("cli; hlt") } }
}

/// Handler pour Page Fault (#PF)
#[no_mangle]
extern "C" fn page_fault_handler(stack_frame: &InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    
    serial_println!("[#PF] Page Fault!");
    serial_println!("  Address: {:#x}", cr2);
    serial_println!("  Error Code: {:#b}", error_code);
    serial_println!("  RIP: {:#x}", stack_frame.instruction_pointer);
    
    // Pour l'instant, on halt
    loop { unsafe { asm!("hlt") } }
}

/// Handler pour Timer (IRQ 0)
#[no_mangle]
extern "C" fn timer_interrupt_handler(_stack_frame: &InterruptStackFrame) {
    // IMPORTANT: Envoyer EOI au PIC
    unsafe {
        // EOI au PIC Master
        asm!("out 0x20, al", in("al") 0x20u8, options(nomem, nostack));
    }
    
    // TODO: Incrémenter un compteur de ticks
    // TODO: Appeler le scheduler
}

/// Handler pour Clavier (IRQ 1)
#[no_mangle]
extern "C" fn keyboard_interrupt_handler(_stack_frame: &InterruptStackFrame) {
    // Lire le scancode
    let scancode: u8;
    unsafe {
        asm!("in al, 0x60", out("al") scancode, options(nomem, nostack));
    }
    
    serial_println!("[KEYBOARD] Scancode: {:#x}", scancode);
    
    // EOI au PIC Master
    unsafe {
        asm!("out 0x20, al", in("al") 0x20u8, options(nomem, nostack));
    }
}

// ============================================================================
// GÉNÉRATION DES WRAPPERS ASM
// ============================================================================

// Exceptions sans error code
interrupt_handler!(division_error_wrapper, division_error_handler);
interrupt_handler!(breakpoint_wrapper, breakpoint_handler);

// Exceptions avec error code (nécessitent un traitement spécial)
#[naked]
pub unsafe extern "C" fn double_fault_wrapper() {
    unsafe {
        asm!(
            // 1. Le CPU a pushé l'error code AVANT le stack frame
            // Stack layout: [error_code] [stack_frame]
            
            // 2. Sauvegarder les registres
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
            
            // 3. Arguments: rdi=stack_frame, rsi=error_code
            "mov rdi, rsp",
            "add rdi, 15*8 + 8",  // Sauter registres + error_code
            "mov rsi, [rsp + 15*8]",  // Lire error_code
            
            // 4. Appeler le handler
            "call double_fault_handler",
            
            // 5. Restaurer et retourner (ne devrait jamais arriver ici)
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
            "iretq",
            
            options(noreturn)
        )
    }
}

#[naked]
pub unsafe extern "C" fn page_fault_wrapper() {
    unsafe {
        asm!(
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
            "iretq",
            
            options(noreturn)
        )
    }
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

// ============================================================================
// HELPER POUR SERIAL OUTPUT (à adapter selon votre implémentation)
// ============================================================================

macro_rules! serial_println {
    ($($arg:tt)*) => {
        // TODO: Implémenter l'envoi vers le port série
        // Pour l'instant, on ne fait rien pour éviter les dépendances
    };
}
