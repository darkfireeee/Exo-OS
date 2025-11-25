//! Physical memory management

pub mod bitmap_allocator;
pub mod buddy_allocator;
pub mod frame;
pub mod numa;
pub mod zone;

use crate::memory::{PhysicalAddress, MemoryError, MemoryResult};

/// Taille d'une frame physique (4KB)
pub const FRAME_SIZE: usize = 4096;

/// Représente une frame de mémoire physique (4KB)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub start: PhysicalAddress,
}

impl Frame {
    /// Crée une frame à partir d'une adresse physique
    pub fn new(addr: PhysicalAddress) -> Self {
        Frame { start: addr }
    }
    
    /// Retourne la frame contenant l'adresse donnée
    pub const fn containing_address(addr: PhysicalAddress) -> Self {
        Frame { start: PhysicalAddress::new(addr.value() & !0xFFF) }
    }
    
    /// Retourne l'adresse de début de la frame
    pub const fn address(&self) -> PhysicalAddress {
        self.start
    }

    /// Retourne l'adresse de fin de la frame (exclusive)
    pub const fn end_address(&self) -> PhysicalAddress {
        PhysicalAddress::new(self.start.value() + FRAME_SIZE)
    }

    /// Retourne un range d'adresses pour cette frame
    pub const fn range(&self) -> core::ops::Range<usize> {
        self.start.value()..(self.start.value() + FRAME_SIZE)
    }
}

/// Alloue une frame physique
pub fn allocate_frame() -> MemoryResult<Frame> {
    bitmap_allocator::allocate_frame()
}

/// Alloue plusieurs frames contiguës
pub fn allocate_contiguous_frames(count: usize) -> MemoryResult<Frame> {
    bitmap_allocator::allocate_contiguous_frames(count)
}

/// Libère une frame physique
pub fn deallocate_frame(frame: Frame) -> MemoryResult<()> {
    bitmap_allocator::deallocate_frame(frame)
}

/// Initialise l'allocateur de frames physiques
/// 
/// # Safety
/// Doit être appelé une seule fois au démarrage avec des paramètres valides
pub unsafe fn init_frame_allocator(
    bitmap_addr: usize,
    bitmap_size: usize,
    base_addr: PhysicalAddress,
    total_memory: usize,
) {
    bitmap_allocator::init_global_allocator(bitmap_addr, bitmap_size, base_addr, total_memory);
}

/// Marque une région de mémoire comme utilisée
pub fn mark_region_used(start: PhysicalAddress, size: usize) {
    if let Some(ref mut allocator) = *bitmap_allocator::FRAME_ALLOCATOR.lock() {
        allocator.mark_region_used(start, size);
    }
}

/// Retourne les statistiques de l'allocateur
pub fn get_allocator_stats() -> Option<bitmap_allocator::AllocatorStats> {
    bitmap_allocator::get_stats()
}
