//! Allocateur de frames physiques
//! 
//! Ce module implémente un allocateur de frames physique simple mais fonctionnel
//! pour le démarrage du kernel.

use crate::println;
use x86_64::{
    structures::paging::{FrameAllocator, PhysFrame, Size4KiB},
    PhysAddr,
};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Structure représentant l'allocateur de frames physiques
pub struct BitmapFrameAllocator {
    /// Nombre total de frames disponibles
    total_frames: usize,
    /// Index de la prochaine frame libre
    next_free: AtomicUsize,
    /// Bitmap des frames libres (1 = libre, 0 = occupée)
    free_frames: &'static mut [u8],
    /// Adresse de base des frames
    base_addr: usize,
}

impl BitmapFrameAllocator {
    /// Crée un nouvel allocateur de frames
    pub fn init(start_addr: usize, size: usize) -> Self {
        let frame_size = 4096; // Size4KiB::SIZE
        let total_frames = size / frame_size;
        
        // Créer la bitmap (1 bit par frame, arrondi au byte supérieur)
        let bitmap_size = (total_frames + 7) / 8;
        
        println!("[FRAME_ALLOC] Initialisation:");
        println!("  Adresse de base: 0x{:x}", start_addr);
        println!("  Taille: {} bytes ({})", size, size / 1024 / 1024);
        println!("  Frames totales: {}", total_frames);
        println!("  Bitmap size: {} bytes", bitmap_size);
        
        // Marquer les premières frames comme occupées (pour la bitmap elle-même)
        let bitmap_frames = (bitmap_size + frame_size - 1) / frame_size;
        
        let mut allocator = Self {
            total_frames,
            next_free: AtomicUsize::new(bitmap_frames), // Commencer après la bitmap
            free_frames: unsafe { core::slice::from_raw_parts_mut(
                start_addr as *mut u8, 
                bitmap_size
            )},
            base_addr: start_addr,
        };
        
        // Initialiser la bitmap
        allocator.free_frames.fill(0xFF); // Toutes libres
        
        // Marquer les frames utilisées comme occupées
        for frame in 0..bitmap_frames {
            allocator.mark_frame_used(frame);
        }
        
        allocator
    }
    
    /// Crée un allocateur de fallback avec zone fixe
    pub fn init_fallback() -> Self {
        println!("[FRAME_ALLOC] Utilisation du mode fallback");
        // Zone fixe à partir de 1MB (standard pour les kernels)
        let start_addr = 0x100000;
        let size = 16 * 1024 * 1024; // 16MB
        
        Self::init(start_addr, size)
    }
    
    /// Marque une frame comme utilisée
    fn mark_frame_used(&mut self, frame_index: usize) {
        if frame_index < self.total_frames {
            let byte_index = frame_index / 8;
            let bit_index = frame_index % 8;
            self.free_frames[byte_index] &= !(1 << bit_index);
        }
    }
    
    /// Marque une frame comme libre
    fn mark_frame_free(&mut self, frame_index: usize) {
        if frame_index < self.total_frames {
            let byte_index = frame_index / 8;
            let bit_index = frame_index % 8;
            self.free_frames[byte_index] |= 1 << bit_index;
        }
    }
    
    /// Vérifie si une frame est libre
    fn is_frame_free(&self, frame_index: usize) -> bool {
        if frame_index >= self.total_frames {
            return false;
        }
        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;
        (self.free_frames[byte_index] & (1 << bit_index)) != 0
    }
    
    /// Trouve la prochaine frame libre
    fn find_free_frame(&mut self, start: usize) -> Option<usize> {
        for frame in start..self.total_frames {
            if self.is_frame_free(frame) {
                return Some(frame);
            }
        }
        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        loop {
            let current_next = self.next_free.load(Ordering::SeqCst);
            
            if current_next >= self.total_frames {
                return None;
            }
            
            // Chercher une frame libre à partir de current_next
            if let Some(frame_index) = self.find_free_frame(current_next) {
                // Tenter d'atomiquement déplacer next_free
                let next_candidate = frame_index + 1;
                if self.next_free.compare_exchange(
                    current_next,
                    next_candidate,
                    Ordering::SeqCst,
                    Ordering::SeqCst
                ).is_ok() {
                    // Marquer la frame comme occupée
                    self.mark_frame_used(frame_index);
                    
                    // Calculer l'adresse physique
                    let phys_addr = self.base_addr + frame_index * 4096; // Size4KiB::SIZE
                    return Some(PhysFrame::from_start_address(PhysAddr::new(phys_addr as u64)).unwrap());
                }
            } else {
                // Plus de frames libres
                self.next_free.store(self.total_frames, Ordering::SeqCst);
                return None;
            }
        }
    }
}

// Instance globale de l'allocateur
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = 
        Mutex::new(None);
}

/// Initialise l'allocateur de frames
pub fn init(start: *mut u8, size: usize) {
    let mut allocator_opt = FRAME_ALLOCATOR.lock();
    *allocator_opt = Some(BitmapFrameAllocator::init(start as usize, size));
}

/// Initialise l'allocateur de fallback
pub fn init_fallback() {
    let mut allocator_opt = FRAME_ALLOCATOR.lock();
    *allocator_opt = Some(BitmapFrameAllocator::init_fallback());
}

/// Alloue une frame (fonction publique)
pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    let mut allocator_opt = FRAME_ALLOCATOR.lock();
    if let Some(ref mut allocator) = *allocator_opt {
        allocator.allocate_frame()
    } else {
        None
    }
}

/// Libère une frame (fonction publique)
pub fn deallocate_frame(frame: PhysFrame<Size4KiB>) {
    let mut allocator_opt = FRAME_ALLOCATOR.lock();
    if let Some(ref mut allocator) = *allocator_opt {
        let frame_index = (frame.start_address().as_u64() as usize - allocator.base_addr) / 4096; // Size4KiB::SIZE
        if frame_index < allocator.total_frames {
            allocator.mark_frame_free(frame_index);
        }
    }
}