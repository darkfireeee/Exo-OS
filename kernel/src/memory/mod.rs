//! Module de gestion mémoire pour le noyau Exo-Kernel
//! 
//! Ce module fournit une abstraction complète pour la gestion de la mémoire,
//! incluant l'allocation de frames physiques, la gestion des tables de pages
//! et l'allocation de tas pour le noyau.

pub mod frame_allocator;
pub mod page_table;
pub mod heap_allocator;

use x86_64::{
    structures::paging::{
        Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
use crate::println;

/// Taille d'une frame mémoire (4 KiB sur x86_64)
pub const FRAME_SIZE: usize = 4096;

/// Adresse de début de la mémoire physique (définie par le bootloader)
pub const PHYS_MEMORY_OFFSET: u64 = 0xFFFF_8000_0000_0000;

/// Initialise le gestionnaire de mémoire (version simplifiée)
/// 
/// Pour l'instant, cette fonction est un stub car nous n'avons pas encore
/// d'intégration complète avec le bootloader.
pub fn init() {
    println!("[MEMORY] Initialisation du gestionnaire de mémoire...");
    
    // TODO: Implémenter l'initialisation complète avec le bootloader
    // - Récupérer la memory map
    // - Initialiser le frame allocator
    // - Configurer les tables de pages
    // - Initialiser le heap allocator
    
    println!("[MEMORY] Gestionnaire de mémoire initialisé (mode simplifié).");
}

// TODO: Ces fonctions seront implémentées plus tard avec l'intégration bootloader complète