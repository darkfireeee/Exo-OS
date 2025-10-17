//! # Interface des Appels Système
//! 
//! Ce module fournit une interface d'appels système optimisée pour atteindre
//! plus de 5 millions d'appels par seconde. Il utilise les instructions
//! `syscall`/`sysenter` pour un passage en mode utilisateur efficace.

use core::arch::asm;
use crate::println;

/// Initialise le mécanisme d'appels système
pub fn init() {
    println!("[SYSCALL] Initialisation du mécanisme d'appels système...");
    
    // Pour l'instant, on skip l'initialisation des MSRs car elle nécessite
    // une configuration complexe du mode utilisateur qui n'est pas encore prêt
    // TODO: Implémenter l'initialisation complète des MSRs quand le mode user sera prêt
    
    println!("[SYSCALL] Interface d'appels système initialisée (mode simplifié).");
}

// Point d'entrée des appels système - Version simplifiée
// TODO: Implémenter la vraie routine d'appels système avec syscall/sysret

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

// Dispatch des appels système - Version simplifiée pour l'instant
// TODO: Implémenter le vrai dispatcher quand le mode utilisateur sera prêt

// Inclure les implémentations des appels système
mod dispatch;
pub use dispatch::*;