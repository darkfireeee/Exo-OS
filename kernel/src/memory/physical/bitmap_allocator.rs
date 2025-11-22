//! Bitmap-based physical frame allocator
//! 
//! Utilise un bitmap pour suivre l'état des frames physiques (libre/occupé)
//! Chaque bit représente une frame de 4KB

use crate::memory::{PhysicalAddress, MemoryError, MemoryResult};
use super::Frame;
use spin::Mutex;

/// Taille d'une frame physique (4KB)
const FRAME_SIZE: usize = 4096;

/// Allocateur de frames basé sur un bitmap
pub struct BitmapFrameAllocator {
    /// Bitmap des frames (1 bit par frame, 0 = libre, 1 = occupé)
    bitmap: &'static mut [u8],
    /// Nombre total de frames
    total_frames: usize,
    /// Index de la prochaine frame à vérifier (optimisation)
    next_free_hint: usize,
    /// Nombre de frames libres
    free_frames: usize,
    /// Adresse physique de départ
    base_address: PhysicalAddress,
}

impl BitmapFrameAllocator {
    /// Crée un nouvel allocateur vide
    /// 
    /// # Safety
    /// bitmap_addr doit pointer vers une région mémoire valide de taille bitmap_size
    pub unsafe fn new(
        bitmap_addr: usize,
        bitmap_size: usize,
        base_addr: PhysicalAddress,
        total_memory: usize,
    ) -> Self {
        let bitmap = core::slice::from_raw_parts_mut(bitmap_addr as *mut u8, bitmap_size);
        let total_frames = total_memory / FRAME_SIZE;

        // Initialiser le bitmap à 0 (toutes les frames libres)
        for byte in bitmap.iter_mut() {
            *byte = 0;
        }

        BitmapFrameAllocator {
            bitmap,
            total_frames,
            next_free_hint: 0,
            free_frames: total_frames,
            base_address: base_addr,
        }
    }

    /// Alloue une frame physique
    pub fn allocate(&mut self) -> MemoryResult<Frame> {
        // Chercher une frame libre à partir du hint
        for frame_index in self.next_free_hint..self.total_frames {
            if self.is_free(frame_index) {
                self.mark_used(frame_index);
                self.next_free_hint = frame_index + 1;
                self.free_frames -= 1;
                
                let addr = self.base_address.value() + (frame_index * FRAME_SIZE);
                return Ok(Frame::new(PhysicalAddress::new(addr)));
            }
        }

        // Si pas trouvé, chercher depuis le début jusqu'au hint
        for frame_index in 0..self.next_free_hint {
            if self.is_free(frame_index) {
                self.mark_used(frame_index);
                self.next_free_hint = frame_index + 1;
                self.free_frames -= 1;
                
                let addr = self.base_address.value() + (frame_index * FRAME_SIZE);
                return Ok(Frame::new(PhysicalAddress::new(addr)));
            }
        }

        Err(MemoryError::OutOfMemory)
    }

    /// Alloue plusieurs frames contiguës
    pub fn allocate_contiguous(&mut self, count: usize) -> MemoryResult<Frame> {
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }

        // Chercher 'count' frames contiguës libres
        'outer: for start_frame in 0..(self.total_frames - count + 1) {
            // Vérifier si toutes les frames sont libres
            for offset in 0..count {
                if !self.is_free(start_frame + offset) {
                    continue 'outer;
                }
            }

            // Toutes les frames sont libres, les marquer comme utilisées
            for offset in 0..count {
                self.mark_used(start_frame + offset);
            }

            self.free_frames -= count;
            self.next_free_hint = start_frame + count;

            let addr = self.base_address.value() + (start_frame * FRAME_SIZE);
            return Ok(Frame::new(PhysicalAddress::new(addr)));
        }

        Err(MemoryError::OutOfMemory)
    }

    /// Libère une frame
    pub fn deallocate(&mut self, frame: Frame) -> MemoryResult<()> {
        let addr = frame.address().value();
        let base = self.base_address.value();

        if addr < base {
            return Err(MemoryError::InvalidAddress);
        }

        let offset = addr - base;
        if offset % FRAME_SIZE != 0 {
            return Err(MemoryError::AlignmentError);
        }

        let frame_index = offset / FRAME_SIZE;
        if frame_index >= self.total_frames {
            return Err(MemoryError::InvalidAddress);
        }

        if self.is_free(frame_index) {
            // Double free
            return Err(MemoryError::InternalError("Double free detected"));
        }

        self.mark_free(frame_index);
        self.free_frames += 1;
        
        // Mettre à jour le hint si cette frame est avant
        if frame_index < self.next_free_hint {
            self.next_free_hint = frame_index;
        }

        Ok(())
    }

    /// Marque une région comme utilisée (pour réserver le kernel, etc.)
    pub fn mark_region_used(&mut self, start: PhysicalAddress, size: usize) {
        let base = self.base_address.value();
        let start_addr = start.value();

        if start_addr < base {
            return;
        }

        let offset = start_addr - base;
        let start_frame = offset / FRAME_SIZE;
        let frame_count = (size + FRAME_SIZE - 1) / FRAME_SIZE;

        for i in 0..frame_count {
            let frame_index = start_frame + i;
            if frame_index < self.total_frames && self.is_free(frame_index) {
                self.mark_used(frame_index);
                self.free_frames -= 1;
            }
        }
    }

    /// Vérifie si une frame est libre
    fn is_free(&self, frame_index: usize) -> bool {
        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;

        if byte_index >= self.bitmap.len() {
            return false;
        }

        (self.bitmap[byte_index] & (1 << bit_index)) == 0
    }

    /// Marque une frame comme utilisée
    fn mark_used(&mut self, frame_index: usize) {
        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;

        if byte_index < self.bitmap.len() {
            self.bitmap[byte_index] |= 1 << bit_index;
        }
    }

    /// Marque une frame comme libre
    fn mark_free(&mut self, frame_index: usize) {
        let byte_index = frame_index / 8;
        let bit_index = frame_index % 8;

        if byte_index < self.bitmap.len() {
            self.bitmap[byte_index] &= !(1 << bit_index);
        }
    }

    /// Retourne les statistiques de l'allocateur
    pub fn stats(&self) -> AllocatorStats {
        AllocatorStats {
            total_frames: self.total_frames,
            free_frames: self.free_frames,
            used_frames: self.total_frames - self.free_frames,
            total_memory: self.total_frames * FRAME_SIZE,
            free_memory: self.free_frames * FRAME_SIZE,
            used_memory: (self.total_frames - self.free_frames) * FRAME_SIZE,
        }
    }
}

/// Statistiques de l'allocateur
#[derive(Debug, Clone, Copy)]
pub struct AllocatorStats {
    pub total_frames: usize,
    pub free_frames: usize,
    pub used_frames: usize,
    pub total_memory: usize,
    pub free_memory: usize,
    pub used_memory: usize,
}

/// Instance globale thread-safe de l'allocateur
pub static FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = Mutex::new(None);

/// Initialise l'allocateur global
/// 
/// # Safety
/// Doit être appelé une seule fois au démarrage
pub unsafe fn init_global_allocator(
    bitmap_addr: usize,
    bitmap_size: usize,
    base_addr: PhysicalAddress,
    total_memory: usize,
) {
    let allocator = BitmapFrameAllocator::new(bitmap_addr, bitmap_size, base_addr, total_memory);
    *FRAME_ALLOCATOR.lock() = Some(allocator);
}

/// Alloue une frame via l'allocateur global
pub fn allocate_frame() -> MemoryResult<Frame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Frame allocator not initialized"))?
        .allocate()
}

/// Alloue plusieurs frames contiguës via l'allocateur global
pub fn allocate_contiguous_frames(count: usize) -> MemoryResult<Frame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Frame allocator not initialized"))?
        .allocate_contiguous(count)
}

/// Libère une frame via l'allocateur global
pub fn deallocate_frame(frame: Frame) -> MemoryResult<()> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Frame allocator not initialized"))?
        .deallocate(frame)
}

/// Retourne les statistiques de l'allocateur global
pub fn get_stats() -> Option<AllocatorStats> {
    FRAME_ALLOCATOR.lock().as_ref().map(|a| a.stats())
}