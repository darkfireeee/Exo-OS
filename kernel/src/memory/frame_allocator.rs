use crate::memory::PhysicalAddress;

/// Taille d'une frame (4KB)
pub const FRAME_SIZE: usize = 4096;

/// Représente une frame physique de 4KB
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysFrame {
    start_address: PhysicalAddress,
}

impl PhysFrame {
    /// Crée une frame contenant l'adresse donnée
    pub fn containing_address(addr: PhysicalAddress) -> Self {
        Self {
            start_address: PhysicalAddress::new(addr.value() & !0xFFF),
        }
    }
    
    /// Crée une frame depuis un numéro de frame
    pub fn from_frame_number(number: usize) -> Self {
        Self {
            start_address: PhysicalAddress::new(number * FRAME_SIZE),
        }
    }
    
    /// Retourne l'adresse de début de la frame
    pub fn start_address(&self) -> PhysicalAddress {
        self.start_address
    }
    
    /// Retourne le numéro de la frame
    pub fn frame_number(&self) -> usize {
        self.start_address.value() / FRAME_SIZE
    }
}

/// Trait pour les allocateurs de frames
pub trait FrameAllocator {
    /// Alloue une frame physique
    fn allocate_frame(&mut self) -> Option<PhysFrame>;
}

/// Simple frame allocator for early boot
#[derive(Debug)]
pub struct SimpleFrameAllocator {
    next_frame: PhysicalAddress,
    end_frame: PhysicalAddress,
}

// SimpleFrameAllocator is Send because it only contains PhysAddr which is Send
unsafe impl Send for SimpleFrameAllocator {}

impl SimpleFrameAllocator {
    pub fn new() -> Self {
        Self {
            next_frame: PhysicalAddress::new(0x100000), // Start after 1MB
            end_frame: PhysicalAddress::new(0x10000000), // 256MB default
        }
    }
    
    /// Add a memory range to the allocator
    pub fn add_range(&mut self, start: PhysicalAddress, end: PhysicalAddress) {
        // If this range extends our current range, update it
        if start < self.next_frame {
            self.next_frame = start;
        }
        if end > self.end_frame {
            self.end_frame = end;
        }
    }
}

// Implement FrameAllocator trait for SimpleFrameAllocator
impl FrameAllocator for SimpleFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if self.next_frame >= self.end_frame {
            return None;
        }

        let frame = PhysFrame::containing_address(self.next_frame);
        self.next_frame = PhysicalAddress::new(self.next_frame.value() + FRAME_SIZE);
        Some(frame)
    }
}
