//! Allocateur de tas
//! 
//! Le tas est géré par linked_list_allocator qui est configuré dans lib.rs.
//! Ce module fournit des fonctions utilitaires pour l'allocateur de tas.

use core::ptr::NonNull;
use crate::println;

/// Initialise le tas avec la zone fournie par le bootloader
pub fn init() {
    println!("[HEAP] Allocateur de tas initialisé (via linked_list_allocator).");
}

/// Vérifie l'intégrité du tas (fonction de debug)
pub fn check_heap_integrity() -> bool {
    // TODO: Implémenter la vérification de l'intégrité du tas
    // linked_list_allocator ne fournit pas encore cette fonctionnalité
    true
}

/// Obtient des statistiques du tas
pub fn get_stats() -> HeapStats {
    // TODO: Obtenir les vraies statistiques depuis linked_list_allocator
    HeapStats {
        used: 0,
        free: 0,
        allocated_blocks: 0,
        free_blocks: 0,
    }
}

/// Statistiques du tas
#[derive(Debug, Clone)]
pub struct HeapStats {
    pub used: usize,
    pub free: usize,
    pub allocated_blocks: usize,
    pub free_blocks: usize,
}