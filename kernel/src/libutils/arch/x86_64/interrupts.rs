//! Aide à la gestion des interruptions pour x86_64
//! 
//! Ce module fournit des structures et des fonctions pour gérer les interruptions
//! et les exceptions sur l'architecture x86_64.

use core::fmt;

/// Numéros d'interruption pour x86_64
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptIndex {
    /// Exception de division par zéro
    DivideError = 0,
    /// Exception de débordement
    Overflow = 4,
    /// Exception de violation de protection
    GeneralProtectionFault = 13,
    /// Exception de défaut de page
    PageFault = 14,
    /// Interruption de l'APIAC
    ApicTimer = 32,
    /// Interruption du clavier
    Keyboard = 33,
    /// Interruption du port série COM1
    Serial1 = 36,
    /// Interruption du port série COM2
    Serial2 = 37,
    /// Interruption du port série COM3
    Serial3 = 38,
    /// Interruption du port série COM4
    Serial4 = 39,
    /// Interruption de l'APIAC
    ApicError = 49,
    /// Interruption spurious de l'APIAC
    ApicSpurious = 255,
}

impl InterruptIndex {
    /// Convertit l'index en u8
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Convertit l'index en usize
    pub fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

/// Structure représentant le contexte d'une interruption
#[repr(C)]
#[derive(Debug)]
pub struct InterruptContext {
    /// Registres généraux
    pub r15: usize,
    pub r14: usize,
    pub r13: usize,
    pub r12: usize,
    pub r11: usize,
    pub r10: usize,
    pub r9: usize,
    pub r8: usize,
    pub rdi: usize,
    pub rsi: usize,
    pub rdx: usize,
    pub rcx: usize,
    pub rbx: usize,
    pub rax: usize,
    
    /// Pointeur de pile
    pub rsp: usize,
    
    /// Registres de segment
    pub ss: usize,
    pub cs: usize,
    
    /// Drapeaux du processeur
    pub rflags: usize,
    
    /// Pointeur d'instruction
    pub rip: usize,
    
    /// Code d'erreur (pour certaines exceptions)
    pub error_code: usize,
}

/// Handler d'interruption générique
pub type InterruptHandler = unsafe extern "C" fn(&mut InterruptContext);

/// Gestionnaire d'interruptions
pub struct InterruptManager {
    handlers: [Option<InterruptHandler>; 256],
}

impl InterruptManager {
    /// Crée un nouveau gestionnaire d'interruptions
    pub const fn new() -> Self {
        Self {
            handlers: [None; 256],
        }
    }

    /// Enregistre un handler pour une interruption
    pub fn register_handler(&mut self, index: u8, handler: InterruptHandler) {
        self.handlers[index as usize] = Some(handler);
    }

    /// Désenregistre un handler pour une interruption
    pub fn unregister_handler(&mut self, index: u8) {
        self.handlers[index as usize] = None;
    }

    /// Appelle le handler approprié pour une interruption
    pub fn handle_interrupt(&mut self, index: u8, context: &mut InterruptContext) {
        if let Some(handler) = self.handlers[index as usize] {
            unsafe {
                handler(context);
            }
        } else {
            // Pas de handler enregistré, on affiche un message d'erreur
            self.print_unhandled_interrupt(index, context);
        }
    }

    /// Affiche un message pour une interruption non gérée
    fn print_unhandled_interrupt(&self, index: u8, context: &InterruptContext) {
        #[cfg(feature = "debug")]
        {
            use crate::macros::kprintln;
            
            kprintln!("=== INTERRUPTION NON GÉRÉE ===");
            kprintln!("Index: {}", index);
            kprintln!("RIP: {:#x}", context.rip);
            kprintln!("RSP: {:#x}", context.rsp);
            kprintln!("RFLAGS: {:#x}", context.rflags);
            kprintln!("CS: {:#x}", context.cs);
            kprintln!("SS: {:#x}", context.ss);
            kprintln!("RAX: {:#x}", context.rax);
            kprintln!("RBX: {:#x}", context.rbx);
            kprintln!("RCX: {:#x}", context.rcx);
            kprintln!("RDX: {:#x}", context.rdx);
            kprintln!("RSI: {:#x}", context.rsi);
            kprintln!("RDI: {:#x}", context.rdi);
            kprintln!("R8: {:#x}", context.r8);
            kprintln!("R9: {:#x}", context.r9);
            kprintln!("R10: {:#x}", context.r10);
            kprintln!("R11: {:#x}", context.r11);
            kprintln!("R12: {:#x}", context.r12);
            kprintln!("R13: {:#x}", context.r13);
            kprintln!("R14: {:#x}", context.r14);
            kprintln!("R15: {:#x}", context.r15);
            
            if index == 14 {
                // Page fault, afficher des informations supplémentaires
                let error_code = context.error_code;
                kprintln!("PAGE FAULT - Code d'erreur: {:#x}", error_code);
                kprintln!("  Présent: {}", error_code & 0x1 != 0);
                kprintln!("  Écriture: {}", error_code & 0x2 != 0);
                kprintln!("  User: {}", error_code & 0x4 != 0);
                kprintln!("  Reserved Write: {}", error_code & 0x8 != 0);
                kprintln!("  Instruction Fetch: {}", error_code & 0x10 != 0);
                
                // Lire l'adresse qui a causé le défaut de page
                let fault_addr: usize;
                unsafe {
                    asm!("mov {}, cr2", out(reg) fault_addr);
                }
                kprintln!("  Adresse: {:#x}", fault_addr);
            }
            
            kprintln!("=============================");
        }
    }
}

impl fmt::Debug for InterruptManager {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InterruptManager {{ handlers: [")?;
        for (i, handler) in self.handlers.iter().enumerate() {
            if handler.is_some() {
                write!(f, "{}, ", i)?;
            }
        }
        write!(f, "] }}")
    }
}

/// Active les interruptions matérielles
pub fn enable() {
    crate::arch::x86_64::registers::enable_interrupts();
}

/// Désactive les interruptions matérielles
pub fn disable() {
    crate::arch::x86_64::registers::disable_interrupts();
}

/// Vérifie si les interruptions sont activées
pub fn are_enabled() -> bool {
    crate::arch::x86_64::registers::interrupts_enabled()
}

/// Exécute une closure avec les interruptions désactivées
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let was_enabled = are_enabled();
    if was_enabled {
        disable();
    }
    
    let result = f();
    
    if was_enabled {
        enable();
    }
    
    result
}

/// Envoie un EOI (End Of Interrupt) au contrôleur d'interruptions
pub fn send_eoi() {
    // Pour xAPIC, on écrit dans le registre EOI
    // L'adresse du registre EOI est 0xFEE000B0
    unsafe {
        let eoi_addr = 0xFEE000B0 as *mut u32;
        eoi_addr.write_volatile(0);
    }
}