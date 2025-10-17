//! Allocateur de frames physiques (stub)
//! 
//! Ce module sera implémenté après l'intégration du bootloader

use x86_64::{
    structures::paging::{FrameAllocator, PhysFrame, Size4KiB},
    PhysAddr,
};

/// Structure représentant l'allocateur de frames physiques
pub struct BitmapFrameAllocator {
    next: usize,
}

impl BitmapFrameAllocator {
    /// Crée un nouvel allocateur de frames (stub)
    pub const fn new() -> Self {
        Self { next: 0 }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        // TODO: Implémentation réelle après intégration du bootloader
        None
    }
}
