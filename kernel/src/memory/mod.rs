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

// Stub functions
pub fn init_heap(_start: usize, _size: usize) {
    // TODO: Initialize heap allocator
}

pub fn detect_memory(_boot_info: *const u8) {
    // TODO: Parse multiboot2 memory map
}
