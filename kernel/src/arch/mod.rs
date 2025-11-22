//! Architecture-specific modules

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

// Stub for arch modules
pub mod mmu {
    //! Memory Management Unit stubs
    use crate::memory::{VirtualAddress, PhysicalAddress, Frame, MemoryError};
    
    pub fn map_temporary(_phys: PhysicalAddress) -> Result<VirtualAddress, MemoryError> {
        // Stub pour mapping temporaire - retourne une adresse virtuelle fictive
        Ok(VirtualAddress::new(0xFFFF_FF00_0000_0000))
    }
    
    pub fn unmap_temporary(_virt: VirtualAddress) {
        // Stub pour unmapping temporaire
    }
    
    pub fn enable_paging() {
        // Stub pour activer le paging
    }
    
    pub fn get_page_table_root() -> PhysicalAddress {
        // Stub pour obtenir la racine de la table des pages
        PhysicalAddress::new(0)
    }
    
    pub fn set_page_table_root(_addr: PhysicalAddress) {
        // Stub pour définir la racine de la table des pages
    }
    
    pub fn invalidate_tlb(_addr: VirtualAddress) {
        // Stub pour invalider TLB
    }
    
    pub fn invalidate_tlb_all() {
        // Stub pour invalider tout le TLB
    }
}

// Common architecture functions
pub fn init() -> Result<(), &'static str> {
    #[cfg(target_arch = "x86_64")]
    x86_64::init()
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
