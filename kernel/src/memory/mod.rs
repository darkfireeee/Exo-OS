//! Module de gestion mémoire pour le noyau Exo-Kernel
//! 
//! Ce module fournit une abstraction complète pour la gestion de la mémoire,
//! incluant l'allocation de frames physiques, la gestion des tables de pages
//! et l'allocation de tas pour le noyau.

pub mod frame_allocator;
pub mod page_table;
pub mod heap_allocator;

use core::ops::{Range, RangeInclusive};
use x86_64::{
    structures::paging::{
        FrameAllocator, FrameDeallocator, OffsetPageTable, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
use frame_allocator::BitmapFrameAllocator;
use heap_allocator::BuddyHeapAllocator;

/// Taille d'une frame mémoire (4 KiB sur x86_64)
pub const FRAME_SIZE: usize = 4096;

/// Adresse de début de la mémoire physique (définie par le bootloader)
pub const PHYS_MEMORY_OFFSET: u64 = 0xFFFF_8000_0000_0000;

/// Initialise le gestionnaire de mémoire
/// 
/// # Arguments
/// 
/// * `memory_map` - Carte de la mémoire fournie par le bootloader
/// * `kernel_end` - Adresse de fin du noyau en mémoire
/// 
/// # Returns
/// 
/// Un tuple contenant l'allocateur de frames et l'allocateur de tas
pub fn init(
    memory_map: &impl bootloader::boot_info::MemoryMap,
    kernel_end: VirtAddr,
) -> (BitmapFrameAllocator, BuddyHeapAllocator) {
    // Créer l'allocateur de frames physiques
    let frame_allocator = BitmapFrameAllocator::new(memory_map, kernel_end);
    
    // Créer l'allocateur de tas pour le noyau
    let heap_allocator = BuddyHeapAllocator::new();
    
    (frame_allocator, heap_allocator)
}

/// Initialise les tables de pages pour la mémoire virtuelle
/// 
/// # Arguments
/// 
/// * `frame_allocator` - Référence mutable à l'allocateur de frames
/// 
/// # Returns
/// 
/// Une table de pages offset pour la mémoire virtuelle
pub fn init_page_tables(frame_allocator: &mut impl FrameAllocator<Size4KiB>) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;
    
    let (level_4_table_frame, _) = Cr3::read();
    
    let phys_addr = level_4_table_frame.start_address();
    let virt_addr = VirtAddr::new(phys_addr.as_u64() + PHYS_MEMORY_OFFSET);
    let level_4_table = unsafe { &mut *virt_addr.as_mut_ptr() };
    
    unsafe { OffsetPageTable::new(level_4_table, VirtAddr::new(PHYS_MEMORY_OFFSET)) }
}

/// Active l'écriture exécutable (NX bit) pour améliorer la sécurité
pub fn enable_nx_bit() {
    use x86_64::registers::control::{Efer, EferFlags};
    
    unsafe { Efer::update(|efer| *efer |= EferFlags::NO_EXECUTE_ENABLE) };
}

/// Mappe une page à une frame avec les flags spécifiés
/// 
/// # Arguments
/// 
/// * `page_table` - Référence mutable à la table de pages
/// * `frame_allocator` - Référence mutable à l'allocateur de frames
/// * `page` - Page à mapper
/// * `flags` - Flags de la page
/// 
/// # Returns
/// 
/// `Ok(())` si le mapping a réussi, `Err` sinon
pub fn map_page(
    page_table: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    use x86_64::structures::paging::Mapper;
    
    unsafe {
        page_table
            .map_to(page, frame_allocator.allocate_frame().ok_or("out of memory")?, flags, frame_allocator)
            .map(|_| ()).map_err(|_| "map_to failed")
    }
}

/// Crée une mapping identity pour une plage de frames
/// 
/// # Arguments
/// 
/// * `page_table` - Référence mutable à la table de pages
/// * `frame_allocator` - Référence mutable à l'allocateur de frames
/// * `frame_range` - Plage de frames à mapper
/// * `flags` - Flags de la page
/// 
/// # Returns
/// 
/// `Ok(())` si le mapping a réussi, `Err` sinon
pub fn identity_map(
    page_table: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    frame_range: RangeInclusive<x86_64::PhysFrame>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    use x86_64::structures::paging::{Page, PageRangeInclusive};
    
    for frame in frame_range {
        let page = Page::containing_address(x86_64::VirtAddr::new(frame.start_address().as_u64() + PHYS_MEMORY_OFFSET));
        map_page(page_table, frame_allocator, page, flags)?;
    }
    
    Ok(())
}