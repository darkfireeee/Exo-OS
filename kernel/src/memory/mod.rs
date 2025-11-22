//! Memory management subsystem

pub mod heap;
pub mod physical;
pub mod virtual_mem;
pub mod dma;
pub mod mmap;
pub mod protection;
pub mod shared;
pub mod address;

// Re-exports
pub use heap::LockedHeap;
pub use address::{PhysicalAddress, VirtualAddress};
pub use protection::PageProtection;
pub use physical::Frame;

// Error type for memory operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    OutOfMemory,
    InvalidAddress,
    AlreadyMapped,
    NotMapped,
    PermissionDenied,
    AlignmentError,
    InvalidSize,
    InternalError(&'static str),
}

impl core::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            MemoryError::OutOfMemory => write!(f, "Out of memory"),
            MemoryError::InvalidAddress => write!(f, "Invalid address"),
            MemoryError::AlreadyMapped => write!(f, "Already mapped"),
            MemoryError::NotMapped => write!(f, "Not mapped"),
            MemoryError::PermissionDenied => write!(f, "Permission denied"),
            MemoryError::AlignmentError => write!(f, "Alignment error"),
            MemoryError::InvalidSize => write!(f, "Invalid size"),
            MemoryError::InternalError(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

pub type MemoryResult<T> = Result<T, MemoryError>;

/// Configuration de la mémoire pour l'initialisation
pub struct MemoryConfig {
    /// Adresse de début du heap
    pub heap_start: usize,
    /// Taille du heap
    pub heap_size: usize,
    /// Adresse du bitmap pour le frame allocator
    pub bitmap_addr: usize,
    /// Taille du bitmap
    pub bitmap_size: usize,
    /// Adresse physique de base
    pub physical_base: usize,
    /// Taille totale de la mémoire physique
    pub total_memory: usize,
}

impl MemoryConfig {
    /// Configuration par défaut pour le démarrage
    pub fn default_config() -> Self {
        // Configuration pour mémoire identity-mapped (0-1GB)
        // Le bootloader map les premiers 1GB avec huge pages
        const HEAP_START: usize = 0x0080_0000;  // 8MB (après le kernel à ~4MB)
        const HEAP_SIZE: usize = 10 * 1024 * 1024;  // 10MB de heap
        const BITMAP_START: usize = 0x0050_0000;  // 5MB (juste après le kernel)
        const TOTAL_MEMORY: usize = 64 * 1024 * 1024;  // 64MB utilisables
        const BITMAP_SIZE: usize = TOTAL_MEMORY / 4096 / 8;  // 1 bit par frame de 4KB = 2KB

        MemoryConfig {
            heap_start: HEAP_START,
            heap_size: HEAP_SIZE,
            bitmap_addr: BITMAP_START,
            bitmap_size: BITMAP_SIZE,
            physical_base: 0,
            total_memory: TOTAL_MEMORY,
        }
    }
}

/// Initialise le système de gestion de la mémoire
/// 
/// Cette fonction doit être appelée tôt dans le boot process.
/// Elle initialise:
/// - L'allocateur de frames physiques (bitmap)
/// - Le heap allocator
/// - Marque les régions utilisées (kernel, multiboot, etc.)
pub fn init(config: MemoryConfig) -> MemoryResult<()> {
    // 1. Initialiser le frame allocator
    unsafe {
        physical::init_frame_allocator(
            config.bitmap_addr,
            config.bitmap_size,
            PhysicalAddress::new(config.physical_base),
            config.total_memory,
        );
    }

    // 2. Marquer les régions réservées
    // Réserver les premiers 1MB (BIOS, VGA, etc.)
    physical::mark_region_used(PhysicalAddress::new(0), 0x100000);

    // Réserver le kernel (1MB - 4MB approximativement)
    physical::mark_region_used(PhysicalAddress::new(0x100000), 3 * 1024 * 1024);

    // Réserver le bitmap
    physical::mark_region_used(
        PhysicalAddress::new(config.bitmap_addr),
        config.bitmap_size,
    );

    // Réserver le heap
    physical::mark_region_used(
        PhysicalAddress::new(config.heap_start),
        config.heap_size,
    );

    // 3. Initialiser le heap allocator
    unsafe {
        crate::ALLOCATOR.init(config.heap_start, config.heap_size);
    }

    Ok(())
}

/// Initialise le heap avec une région mémoire spécifique
/// 
/// # Safety
/// La région [start, start + size) doit être valide et non utilisée
pub unsafe fn init_heap(start: usize, size: usize) {
    crate::ALLOCATOR.init(start, size);
}

/// Détecte la mémoire disponible depuis les informations multiboot
pub fn detect_memory(_boot_info: *const u8) -> MemoryConfig {
    // TODO: Parser les informations multiboot pour détecter la mémoire réelle
    // Pour l'instant, retourner la configuration par défaut
    MemoryConfig::default_config()
}
