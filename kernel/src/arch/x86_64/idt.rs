//! Interrupt Descriptor Table (IDT)
//! 
//! L'IDT définit les gestionnaires pour les 256 vecteurs d'interruption possibles:
//! - 0-31: Exceptions CPU (divide error, page fault, etc.)
//! - 32-47: Interruptions matérielles (IRQ 0-15 du PIC)
//! - 48-255: Interruptions logicielles et réservées

#![allow(unsafe_attr_outside_unsafe)]

/// Nombre d'entrées dans l'IDT (256 vecteurs d'interruption)
const IDT_ENTRIES: usize = 256;

/// Structure d'une entrée IDT (16 bytes en mode 64-bit)
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct IdtEntry {
    offset_low: u16,      // Bits 0-15 de l'adresse du handler
    selector: u16,        // Sélecteur de segment de code (GDT)
    ist: u8,              // Interrupt Stack Table offset (0 = pas d'IST)
    type_attr: u8,        // Type et attributs (P, DPL, type de gate)
    offset_mid: u16,      // Bits 16-31 de l'adresse du handler
    offset_high: u32,     // Bits 32-63 de l'adresse du handler
    reserved: u32,        // Réservé (doit être 0)
}

impl IdtEntry {
    /// Crée une entrée IDT vide
    const fn new() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }
    
    /// Alias pour new() pour compatibilité
    const fn empty() -> Self {
        Self::new()
    }

    /// Configure une entrée IDT pour un gestionnaire d'interruption
    /// 
    /// # Arguments
    /// * `handler` - Adresse de la fonction de gestion d'interruption
    /// * `selector` - Sélecteur de segment de code (généralement 0x08 pour kernel code)
    /// * `ist` - Index dans l'Interrupt Stack Table (0 = utilise la pile actuelle)
    /// * `type_attr` - Type de gate et attributs (0x8E = present, ring 0, interrupt gate)
    fn set_handler(&mut self, handler: usize, selector: u16, ist: u8, type_attr: u8) {
        self.offset_low = (handler & 0xFFFF) as u16;
        self.offset_mid = ((handler >> 16) & 0xFFFF) as u16;
        self.offset_high = ((handler >> 32) & 0xFFFFFFFF) as u32;
        self.selector = selector;
        self.ist = ist;
        self.type_attr = type_attr;
        self.reserved = 0;
    }
}

/// Structure de l'IDT complète (256 entrées)
#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; IDT_ENTRIES],
}

// Impl Idt supprimé - on utilise directement la statique IDT

/// Structure du registre IDTR (chargé avec LIDT)
#[repr(C, packed)]
struct IdtPointer {
    limit: u16,  // Taille de l'IDT - 1
    base: u64,   // Adresse de base de l'IDT
}

/// Instance globale de l'IDT (statique, toujours en mémoire)
static mut IDT: Idt = Idt {
    entries: [IdtEntry::empty(); IDT_ENTRIES],
};

/// Initialise l'IDT et charge-la dans le CPU
pub fn init() {
    unsafe {
        // Récupérer les adresses des handlers
        let handlers = super::handlers::get_handler_addresses();
        
        // Configurer les handlers essentiels
        // Code segment selector = 0x08 (GDT entry 1)
        
        // #DE (0) - Division Error
        IDT.entries[0].set_handler(handlers.division_error, 0x08, 0, 0x8E);
        
        // #BP (3) - Breakpoint (DPL=3 pour userspace)
        IDT.entries[3].set_handler(handlers.breakpoint, 0x08, 0, 0xEE);
        
        // #DF (8) - Double Fault (IST=0 pour l'instant)
        IDT.entries[8].set_handler(handlers.double_fault, 0x08, 0, 0x8E);
        
        // #PF (14) - Page Fault
        IDT.entries[14].set_handler(handlers.page_fault, 0x08, 0, 0x8E);
        
        // IRQ 0 (32) - Timer
        IDT.entries[32].set_handler(handlers.timer, 0x08, 0, 0x8E);
        
        // IRQ 1 (33) - Keyboard
        IDT.entries[33].set_handler(handlers.keyboard, 0x08, 0, 0x8E);
        
        // Charger l'IDT dans le CPU
        let idtr = IdtPointer {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: &IDT as *const _ as u64,
        };
        
        core::arch::asm!(
            "lidt [{}]", 
            in(reg) &idtr, 
            options(readonly, nostack, preserves_flags)
        );
    }
}

//
// Gestionnaires d'exceptions CPU (0-31)
//

/// Handler ASM simple pour Division par zéro (Exception #0)
/// Utilise naked function avec affichage VGA direct
#[unsafe(naked)]
#[no_mangle]
extern "C" fn divide_error_handler() {
    core::arch::naked_asm!(
            // Sauvegarder les registres
            "push rax",
            "push rbx",
            "push rcx",
            
            // Afficher message VGA
            "mov rax, 0xB8000",              // Adresse VGA
            "add rax, {row_offset}",          // Ligne 24
            "mov word ptr [rax + 0], 0x4F5B", // '['
            "mov word ptr [rax + 2], 0x4F45", // 'E'
            "mov word ptr [rax + 4], 0x4F58", // 'X'
            "mov word ptr [rax + 6], 0x4F43", // 'C'
            "mov word ptr [rax + 8], 0x4F45", // 'E'
            "mov word ptr [rax + 10], 0x4F50", // 'P'
            "mov word ptr [rax + 12], 0x4F5D", // ']'
            "mov word ptr [rax + 14], 0x4F20", // ' '
            "mov word ptr [rax + 16], 0x4F44", // 'D'
            "mov word ptr [rax + 18], 0x4F69", // 'i'
            "mov word ptr [rax + 20], 0x4F76", // 'v'
            "mov word ptr [rax + 22], 0x4F20", // ' '
            "mov word ptr [rax + 24], 0x4F62", // 'b'
            "mov word ptr [rax + 26], 0x4F79", // 'y'
            "mov word ptr [rax + 28], 0x4F20", // ' '
            "mov word ptr [rax + 30], 0x4F30", // '0'
            
            // Restaurer les registres
            "pop rcx",
            "pop rbx",
            "pop rax",
            
            // Retour d'interruption
            "iretq",
            
            row_offset = const (24 * 80 * 2)
        );
}

/*
// Anciens handlers naked commentés pour référence
#[unsafe(naked)]
extern "C" fn divide_error_handler() {
    core::arch::naked_asm!(
        "push 0",                    // Pas de code d'erreur
        "push 0",                    // Numéro d'exception: 0
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn debug_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 1",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn nmi_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 2",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn breakpoint_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 3",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn overflow_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 4",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn bound_range_exceeded_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 5",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn invalid_opcode_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 6",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn device_not_available_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 7",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn double_fault_handler() {
    core::arch::naked_asm!(
        // Double fault pousse déjà un code d'erreur
        "push 8",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn coprocessor_segment_overrun_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 9",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn invalid_tss_handler() {
    core::arch::naked_asm!(
        "push 10",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn segment_not_present_handler() {
    core::arch::naked_asm!(
        "push 11",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn stack_segment_fault_handler() {
    core::arch::naked_asm!(
        "push 12",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn general_protection_fault_handler() {
    core::arch::naked_asm!(
        "push 13",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn page_fault_handler() {
    core::arch::naked_asm!(
        "push 14",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn x87_fpu_error_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 16",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn alignment_check_handler() {
    core::arch::naked_asm!(
        "push 17",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn machine_check_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 18",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn simd_floating_point_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 19",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn virtualization_exception_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 20",
        "jmp {common_handler}",
        common_handler = sym exception_common_handler
    );
}

//
// Gestionnaires d'interruptions matérielles
//

#[unsafe(naked)]
extern "C" fn default_irq_handler() {
    core::arch::naked_asm!(
        "push 0",                    // Pas de code d'erreur
        "push 32",                   // Numéro IRQ arbitraire
        "jmp {common_handler}",
        common_handler = sym irq_common_handler
    );
}

#[unsafe(naked)]
extern "C" fn default_interrupt_handler() {
    core::arch::naked_asm!(
        "push 0",
        "push 255",
        "jmp {common_handler}",
        common_handler = sym irq_common_handler
    );
}

//
// Gestionnaires communs
//

/// Structure représentant l'état du CPU sauvegardé lors d'une interruption
#[repr(C)]
struct InterruptFrame {
    // Registres poussés manuellement
    r15: u64, r14: u64, r13: u64, r12: u64,
    r11: u64, r10: u64, r9: u64, r8: u64,
    rdi: u64, rsi: u64, rbp: u64, rdx: u64,
    rcx: u64, rbx: u64, rax: u64,
    
    // Poussés par nos gestionnaires
    int_num: u64,
    error_code: u64,
    
    // Poussés automatiquement par le CPU
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// Gestionnaire commun pour toutes les exceptions
#[unsafe(naked)]
extern "C" fn exception_common_handler() {
    core::arch::naked_asm!(
            // Sauvegarder tous les registres
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
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
            
            // Appeler le gestionnaire Rust
            "mov rdi, rsp",              // Premier argument: pointeur vers InterruptFrame
            "call {handler}",
            
            // Restaurer tous les registres
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
            "pop rdx",
            "pop rcx",
            "pop rbx",
            "pop rax",
            
            // Nettoyer le numéro d'interruption et le code d'erreur
            "add rsp, 16",
            
            // Retourner de l'interruption
            "iretq",
            
            handler = sym exception_handler_rust
        );
}

/// Gestionnaire commun pour les IRQ
#[unsafe(naked)]
extern "C" fn irq_common_handler() {
    core::arch::naked_asm!(
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
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
            "call {handler}",
            
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
            "pop rdx",
            "pop rcx",
            "pop rbx",
            "pop rax",
            
            "add rsp, 16",
            "iretq",
            
            handler = sym irq_handler_rust
        );
}

/// Gestionnaire Rust pour les exceptions
extern "C" fn exception_handler_rust(frame: &InterruptFrame) {
    let vga = 0xB8000 as *mut u16;
    
    unsafe {
        // Efface l'écran en rouge
        for i in 0..80*25 {
            vga.add(i).write_volatile(0x4F20); // Fond rouge, espace
        }
        
        // Affiche "EXCEPTION!"
        let msg = b"EXCEPTION!";
        for (i, &byte) in msg.iter().enumerate() {
            vga.add(i).write_volatile(0x4F00 | byte as u16);
        }
        
        // Affiche le numéro d'exception
        let num_str = b"NUM:";
        for (i, &byte) in num_str.iter().enumerate() {
            vga.add(80 + i).write_volatile(0x4F00 | byte as u16);
        }
        write_hex_at(vga.add(80 + 5), frame.int_num);
        
        // Affiche RIP
        let rip_str = b"RIP:";
        for (i, &byte) in rip_str.iter().enumerate() {
            vga.add(160 + i).write_volatile(0x4F00 | byte as u16);
        }
        write_hex_at(vga.add(160 + 5), frame.rip);
        
        // Affiche le code d'erreur
        let err_str = b"ERR:";
        for (i, &byte) in err_str.iter().enumerate() {
            vga.add(240 + i).write_volatile(0x4F00 | byte as u16);
        }
        write_hex_at(vga.add(240 + 5), frame.error_code);
    }
    
    // Halte le système
    crate::arch::x86_64::halt();
}

/// Gestionnaire Rust pour les IRQ
extern "C" fn irq_handler_rust(_frame: &InterruptFrame) {
    // Pour l'instant, on ne fait rien
    // TODO: Dispatcher vers les handlers spécifiques (timer, clavier, etc.)
    
    // Envoyer EOI au PIC
    unsafe {
        crate::arch::x86_64::outb(0x20, 0x20);  // PIC master EOI
    }
}

/// Écrit un nombre hexadécimal à l'écran
unsafe fn write_hex_at(mut ptr: *mut u16, mut num: u64) {
    for i in (0..16).rev() {
        let digit = ((num >> (i * 4)) & 0xF) as u8;
        let ch = if digit < 10 { b'0' + digit } else { b'A' + (digit - 10) };
        ptr.write_volatile(0x4F00 | ch as u16);
        ptr = ptr.add(1);
    }
}
*/



