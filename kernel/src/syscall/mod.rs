//! # Interface des Appels Système
//! 
//! Ce module fournit une interface d'appels système optimisée pour atteindre
//! plus de 5 millions d'appels par seconde. Il utilise les instructions
//! `syscall`/`sysenter` pour un passage en mode utilisateur efficace.

use core::arch::asm;
use x86_64::registers::msr::{IA32_STAR, IA32_LSTAR, IA32_FMASK, IA32_KERNEL_GS_BASE};
use x86_64::structures::idt::InterruptStackFrame;
use crate::c_compat::serial_write_str;

/// Initialise le mécanisme d'appels système
pub fn init() {
    serial_write_str("Initializing syscall interface...\n");
    
    // Configuration des MSRs pour les appels système
    unsafe {
        // IA32_STAR: bits 63:48 = segment kernel CS, bits 47:32 = segment user CS
        // Kernel CS = 0x08, User CS = 0x18 (segments avec RPL=3)
        x86_64::registers::msr::wrmsr(
            IA32_STAR,
            (0x08u64 << 32) | (0x18u64 << 48)
        );
        
        // IA32_LSTAR: adresse du point d'entrée des appels système en mode long
        x86_64::registers::msr::wrmsr(
            IA32_LSTAR,
            syscall_entry as u64
        );
        
        // IA32_FMASK: masque des drapeaux à effacer lors de l'entrée en mode noyau
        // On efface IF (interruptions) et TF (trap flag)
        x86_64::registers::msr::wrmsr(
            IA32_FMASK,
            0x300  // IF (bit 9) et TF (bit 8)
        );
        
        // Activer les appels système (bit 0 de IA32_EFER)
        let efer = x86_64::registers::model_specific::Msr::new(0xC0000080);
        let mut efer_value = efer.read();
        efer_value |= 1;  // SCE (System Call Extensions)
        efer.write(efer_value);
    }
    
    serial_write_str("Syscall interface initialized.\n");
}

/// Point d'entrée des appels système en assembleur
/// 
/// Cette fonction est appelée directement par l'instruction `syscall`.
/// Elle sauvegarde le contexte utilisateur, appelle le gestionnaire approprié,
/// puis restaure le contexte utilisateur.
#[naked]
pub unsafe extern "C" fn syscall_entry() {
    asm!(
        // Sauvegarder les registres utilisateur
        "push rcx",        // Sauvegarder RIP utilisateur
        "push r11",        // Sauvegarder RFLAGS utilisateur
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        
        // Appeler le gestionnaire d'appels système
        "call {dispatch}",
        
        // Restaurer les registres utilisateur
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",        // Restaurer RFLAGS utilisateur
        "pop rcx",        // Restaurer RIP utilisateur
        
        // Retourner au mode utilisateur avec l'instruction sysret
        "sysretq",
        dispatch = sym dispatch,
        options(noreturn)
    );
}

/// Numéros des appels système
#[repr(u64)]
pub enum SyscallNumber {
    Read = 0,
    Write = 1,
    Open = 2,
    Close = 3,
    Exit = 60,
    Yield = 61,
    Sleep = 62,
    Mmap = 63,
    Munmap = 64,
    Clone = 65,
    IPCSend = 66,
    IPCRecv = 67,
}

/// Structure pour les arguments des appels système
#[repr(C)]
pub struct SyscallArgs {
    pub rdi: u64,  // Premier argument
    pub rsi: u64,  // Deuxième argument
    pub rdx: u64,  // Troisième argument
    pub r10: u64,  // Quatrième argument
    pub r8: u64,   // Cinquième argument
    pub r9: u64,   // Sixième argument
}

/// Dispatch les appels système vers les gestionnaires appropriés
/// 
/// Cette fonction est appelée depuis le point d'entrée en assembleur.
/// Elle utilise le numéro dans RAX pour déterminer quel appel système
/// exécuter et passe les arguments depuis les autres registres.
#[no_mangle]
pub extern "C" fn dispatch() -> u64 {
    // RAX contient le numéro de l'appel système
    let syscall_number;
    unsafe {
        asm!(
            "mov {}, rax",
            out(reg) syscall_number,
        );
    }
    
    // Récupérer les arguments
    let args = SyscallArgs {
        rdi: get_rdi(),
        rsi: get_rsi(),
        rdx: get_rdx(),
        r10: get_r10(),
        r8: get_r8(),
        r9: get_r9(),
    };
    
    // Dispatch vers le gestionnaire approprié
    match syscall_number {
        x if x == SyscallNumber::Read as u64 => sys_read(args),
        x if x == SyscallNumber::Write as u64 => sys_write(args),
        x if x == SyscallNumber::Open as u64 => sys_open(args),
        x if x == SyscallNumber::Close as u64 => sys_close(args),
        x if x == SyscallNumber::Exit as u64 => sys_exit(args),
        x if x == SyscallNumber::Yield as u64 => sys_yield(args),
        x if x == SyscallNumber::Sleep as u64 => sys_sleep(args),
        x if x == SyscallNumber::Mmap as u64 => sys_mmap(args),
        x if x == SyscallNumber::Munmap as u64 => sys_munmap(args),
        x if x == SyscallNumber::Clone as u64 => sys_clone(args),
        x if x == SyscallNumber::IPCSend as u64 => sys_ipc_send(args),
        x if x == SyscallNumber::IPCRecv as u64 => sys_ipc_recv(args),
        _ => {
            // Appel système inconnu
            serial_write_str("Unknown syscall: ");
            // TODO: Convertir le numéro en chaîne pour l'affichage
            serial_write_str("\n");
            0xFFFFFFFFFFFFFFFF  // Code d'erreur
        }
    }
}

// Fonctions utilitaires pour récupérer les valeurs des registres
#[inline(always)]
fn get_rdi() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, rdi",
            out(reg) result,
        );
    }
    result
}

#[inline(always)]
fn get_rsi() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, rsi",
            out(reg) result,
        );
    }
    result
}

#[inline(always)]
fn get_rdx() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, rdx",
            out(reg) result,
        );
    }
    result
}

#[inline(always)]
fn get_r10() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, r10",
            out(reg) result,
        );
    }
    result
}

#[inline(always)]
fn get_r8() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, r8",
            out(reg) result,
        );
    }
    result
}

#[inline(always)]
fn get_r9() -> u64 {
    let result;
    unsafe {
        asm!(
            "mov {}, r9",
            out(reg) result,
        );
    }
    result
}

// Inclure les implémentations des appels système
mod dispatch;
pub use dispatch::*;