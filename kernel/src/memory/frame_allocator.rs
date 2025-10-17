//! Allocateur de frames physiques utilisant une bitmap
//! 
//! Cet allocateur gère les frames de mémoire physique en utilisant une bitmap
//! pour suivre les frames allouées et libres. Il est optimisé pour les opérations
//! rapides d'allocation et de libération.

use bootloader::boot_info::MemoryMap;
use bootloader::boot_info::MemoryRegion;
use core::ops::Range;
use x86_64::{
    structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB},
    PhysAddr,
};
use crate::memory::FRAME_SIZE;

/// Nombre de bits par mot dans la bitmap
const BITS_PER_WORD: usize = 64;

/// Structure représentant l'allocateur de frames physiques
pub struct BitmapFrameAllocator {
    /// Bitmap pour suivre les frames allouées/libres
    bitmap: &'static mut [u64],
    /// Nombre total de frames disponibles
    total_frames: usize,
    /// Première frame gérée par cet allocateur
    start_frame: PhysFrame,
}

impl BitmapFrameAllocator {
    /// Crée un nouvel allocateur de frames à partir de la carte mémoire
    /// 
    /// # Arguments
    /// 
    /// * `memory_map` - Carte de la mémoire fournie par le bootloader
    /// * `kernel_end` - Adresse de fin du noyau en mémoire
    /// 
    /// # Returns
    /// 
/// Une nouvelle instance de `BitmapFrameAllocator`
    pub fn new(memory_map: &MemoryMap, kernel_end: x86_64::VirtAddr) -> Self {
        // Trouver la région de mémoire disponible la plus grande
        let mut largest_region = None;
        let mut largest_size = 0;
        
        for region in memory_map.iter() {
            if region.kind == MemoryRegionKind::Usable {
                let size = region.range.end_addr() - region.range.start_addr();
                if size > largest_size {
                    largest_size = size;
                    largest_region = Some(region);
                }
            }
        }
        
        let largest_region = largest_region.expect("no usable memory region found");
        
        // Calculer le nombre total de frames
        let total_frames = (largest_region.range.end_addr() - largest_region.range.start_addr()) / FRAME_SIZE as u64;
        
        // Calculer la taille de la bitmap nécessaire
        let bitmap_size = (total_frames + BITS_PER_WORD as u64 - 1) / BITS_PER_WORD as u64;
        
        // Placer la bitmap juste après le noyau
        let bitmap_start = kernel_end.align_up(FRAME_SIZE as u64);
        let bitmap_end = bitmap_start + (bitmap_size * 8) as u64;
        
        // S'assurer que la bitmap ne dépasse pas la région de mémoire
        if bitmap_end > largest_region.range.end_addr() {
            panic!("not enough memory for frame allocator bitmap");
        }
        
        // Initialiser la bitmap
        let bitmap = unsafe {
            core::slice::from_raw_parts_mut(
                bitmap_start.as_mut_ptr(),
                bitmap_size as usize,
            )
        };
        
        // Marquer toutes les frames comme utilisées
        for word in bitmap.iter_mut() {
            *word = u64::MAX;
        }
        
        // Marquer les frames utilisables comme libres
        for region in memory_map.iter() {
            if region.kind == MemoryRegionKind::Usable {
                let start_frame = PhysFrame::containing_address(PhysAddr::new(region.range.start_addr()));
                let end_frame = PhysFrame::containing_address(PhysAddr::new(region.range.end_addr() - 1));
                
                for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
                    Self::set_frame_state(bitmap, frame, false);
                }
            }
        }
        
        // Marquer les frames utilisées par la bitmap comme utilisées
        let bitmap_start_frame = PhysFrame::containing_address(PhysAddr::new(bitmap_start.as_u64()));
        let bitmap_end_frame = PhysFrame::containing_address(PhysAddr::new(bitmap_end - 1));
        
        for frame in PhysFrame::range_inclusive(bitmap_start_frame, bitmap_end_frame) {
            Self::set_frame_state(bitmap, frame, true);
        }
        
        Self {
            bitmap,
            total_frames: total_frames as usize,
            start_frame: PhysFrame::containing_address(PhysAddr::new(largest_region.range.start_addr())),
        }
    }
    
    /// Définit l'état d'une frame (utilisée/libre) dans la bitmap
    /// 
    /// # Arguments
    /// 
    /// * `bitmap` - Référence mutable à la bitmap
    /// * `frame` - Frame à modifier
    /// * `used` - `true` si la frame est utilisée, `false` sinon
    fn set_frame_state(bitmap: &mut [u64], frame: PhysFrame, used: bool) {
        let frame_index = Self::frame_index(frame);
        let word_index = frame_index / BITS_PER_WORD;
        let bit_index = frame_index % BITS_PER_WORD;
        
        if used {
            bitmap[word_index] |= 1 << bit_index;
        } else {
            bitmap[word_index] &= !(1 << bit_index);
        }
    }
    
    /// Vérifie si une frame est utilisée
    /// 
    /// # Arguments
    /// 
    /// * `bitmap` - Référence à la bitmap
    /// * `frame` - Frame à vérifier
    /// 
    /// # Returns
    /// 
    /// `true` si la frame est utilisée, `false` sinon
    fn is_frame_used(bitmap: &[u64], frame: PhysFrame) -> bool {
        let frame_index = Self::frame_index(frame);
        let word_index = frame_index / BITS_PER_WORD;
        let bit_index = frame_index % BITS_PER_WORD;
        
        (bitmap[word_index] >> bit_index) & 1 == 1
    }
    
    /// Calcule l'index d'une frame dans la bitmap
    /// 
    /// # Arguments
    /// 
    /// * `frame` - Frame dont on veut l'index
    /// 
    /// # Returns
    /// 
    /// L'index de la frame dans la bitmap
    fn frame_index(frame: PhysFrame) -> usize {
        (frame.start_address().as_u64() / FRAME_SIZE as u64) as usize
    }
    
    /// Trouve la première frame libre dans la bitmap
    /// 
    /// # Returns
    /// 
    /// L'index de la première frame libre, ou `None` si aucune frame n'est libre
    fn find_first_free(&self) -> Option<usize> {
        for (word_index, word) in self.bitmap.iter().enumerate() {
            if *word != u64::MAX {
                // Il y a au moins un bit à 0 dans ce mot
                for bit_index in 0..BITS_PER_WORD {
                    if (*word >> bit_index) & 1 == 0 {
                        return Some(word_index * BITS_PER_WORD + bit_index);
                    }
                }
            }
        }
        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame_index = self.find_first_free()?;
        
        // Marquer la frame comme utilisée
        let word_index = frame_index / BITS_PER_WORD;
        let bit_index = frame_index % BITS_PER_WORD;
        self.bitmap[word_index] |= 1 << bit_index;
        
        // Calculer l'adresse physique de la frame
        let frame_addr = self.start_frame.start_address() + (frame_index as u64) * FRAME_SIZE as u64;
        
        Some(PhysFrame::containing_address(frame_addr))
    }
}

unsafe impl FrameDeallocator<Size4KiB> for BitmapFrameAllocator {
    fn deallocate_frame(&mut self, frame: PhysFrame) {
        let frame_index = Self::frame_index(frame);
        
        // Vérifier que la frame est bien gérée par cet allocateur
        if frame_index >= self.total_frames {
            panic!("attempting to deallocate frame outside of managed range");
        }
        
        // Marquer la frame comme libre
        let word_index = frame_index / BITS_PER_WORD;
        let bit_index = frame_index % BITS_PER_WORD;
        self.bitmap[word_index] &= !(1 << bit_index);
    }
}

/// Extension trait pour ajouter des méthodes utiles à `MemoryRegionKind`
trait MemoryRegionKindExt {
    /// Convertit un `MemoryRegionKind` en notre type interne
    fn to_internal(&self) -> MemoryRegionKind;
}

impl MemoryRegionKindExt for bootloader::boot_info::MemoryRegionKind {
    fn to_internal(&self) -> MemoryRegionKind {
        match self {
            bootloader::boot_info::MemoryRegionKind::Usable => MemoryRegionKind::Usable,
            bootloader::boot_info::MemoryRegionKind::UnknownUefi(_) => MemoryRegionKind::UnknownUefi,
            bootloader::boot_info::MemoryRegionKind::UnknownBios(_) => MemoryRegionKind::UnknownBios,
            bootloader::boot_info::MemoryRegionKind::Reserved => MemoryRegionKind::Reserved,
            bootloader::boot_info::MemoryRegionKind::Framebuffer => MemoryRegionKind::Framebuffer,
        }
    }
}

/// Types de régions mémoire
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryRegionKind {
    Usable,
    UnknownUefi,
    UnknownBios,
    Reserved,
    Framebuffer,
}

impl From<bootloader::boot_info::MemoryRegionKind> for MemoryRegionKind {
    fn from(kind: bootloader::boot_info::MemoryRegionKind) -> Self {
        match kind {
            bootloader::boot_info::MemoryRegionKind::Usable => MemoryRegionKind::Usable,
            bootloader::boot_info::MemoryRegionKind::UnknownUefi(_) => MemoryRegionKind::UnknownUefi,
            bootloader::boot_info::MemoryRegionKind::UnknownBios(_) => MemoryRegionKind::UnknownBios,
            bootloader::boot_info::MemoryRegionKind::Reserved => MemoryRegionKind::Reserved,
            bootloader::boot_info::MemoryRegionKind::Framebuffer => MemoryRegionKind::Framebuffer,
        }
    }
}