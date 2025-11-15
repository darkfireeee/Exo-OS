//! Module de gestion mémoire pour le noyau Exo-Kernel
//! 
//! Ce module fournit une abstraction complète pour la gestion de la mémoire,
//! incluant l'allocation de frames physiques, la gestion des tables de pages
//! et l'allocation de tas pour le noyau.

pub mod frame_allocator;
pub mod page_table;
pub mod heap_allocator;

#[cfg(feature = "hybrid_allocator")]
pub mod hybrid_allocator;

#[cfg(test)]
pub mod bench_allocator;

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

/// Initialise le gestionnaire de mémoire avec intégration bootloader
pub fn init(boot_info: &multiboot2::BootInformation) {
    println!("[MEMORY] Initialisation du gestionnaire de mémoire...");
    
    // Récupérer la memory map du bootloader
    if let Some(memory_map_tag) = boot_info.memory_map_tag() {
        // Compter manuellement les zones mémoire
        let mut areas_count = 0;
        for _area in memory_map_tag.memory_areas() {
            areas_count += 1;
        }
        println!("[MEMORY] Memory map trouvée avec {} zones", areas_count);
        
        // Calculer la mémoire totale disponible
        let mut total_memory = 0u64;
        let mut usable_memory = 0u64;
        
        for area in memory_map_tag.memory_areas() {
            total_memory += area.end_address() - area.start_address();
            if area.typ() == multiboot2::MemoryAreaType::Available {
                usable_memory += area.end_address() - area.start_address();
            }
        }
        
        println!("[MEMORY] Mémoire totale: {} MB", total_memory / 1024 / 1024);
        println!("[MEMORY] Mémoire utilisable: {} MB", usable_memory / 1024 / 1024);
        
        // Initialiser le frame allocator avec la première zone utilisable
        let mut first_usable: Option<_> = None;
        for area in memory_map_tag.memory_areas() {
            if area.typ() == multiboot2::MemoryAreaType::Available {
                first_usable = Some(area);
                break;
            }
        }
        
        if let Some(first_usable) = first_usable {
            
            let start = first_usable.start_address() as usize;
            let end = first_usable.end_address() as usize;
            let size = end - start;
            
            // Réserver les premiers 64K pour le boot
            let heap_start = start + 0x10000;
            let heap_size = (size - 0x10000).min(16 * 1024 * 1024); // max 16MB
            
            unsafe {
                frame_allocator::init(heap_start as *mut u8, heap_size);
            }
            
            println!("[MEMORY] Frame allocator initialisé: 0x{:x} - 0x{:x}", 
                     heap_start, heap_start + heap_size);
        }
    } else {
        println!("[WARNING] Pas de memory map trouvée, utilisation du mode dégradé");
        // Mode dégradé avec allocation statique
        unsafe {
            frame_allocator::init_fallback();
        }
    }
    
    // Initialiser le page table manager
    page_table::init();
    
    // Initialiser le heap allocator
    heap_allocator::init();
    
    println!("[MEMORY] Gestionnaire de mémoire initialisé avec succès.");
}