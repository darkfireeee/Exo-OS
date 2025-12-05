//! Architecture-specific modules

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

/// Memory Management Unit - fonctions réelles pour x86_64
pub mod mmu {
    //! Memory Management Unit pour x86_64
    use crate::memory::{VirtualAddress, PhysicalAddress, MemoryError};
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    /// Zone de mapping temporaire (1024 pages de 4KB = 4MB)
    const TEMP_MAP_BASE: usize = 0xFFFF_FF00_0000_0000;
    const TEMP_MAP_SIZE: usize = 1024;
    
    /// Index de la prochaine page temporaire disponible
    static TEMP_MAP_INDEX: AtomicUsize = AtomicUsize::new(0);
    
    /// Mappe temporairement une adresse physique dans l'espace virtuel
    /// Retourne une adresse virtuelle accessible
    pub fn map_temporary(phys: PhysicalAddress) -> Result<VirtualAddress, MemoryError> {
        // Pour l'instant, on utilise l'identity mapping de boot.asm (0-8GB)
        // Les adresses physiques < 8GB sont déjà mappées à l'identique
        let phys_val = phys.value();
        
        if phys_val < 8 * 1024 * 1024 * 1024 {
            // Identity mapping disponible
            Ok(VirtualAddress::new(phys_val))
        } else {
            // Pour les adresses > 8GB, utiliser la zone temporaire
            let index = TEMP_MAP_INDEX.fetch_add(1, Ordering::SeqCst) % TEMP_MAP_SIZE;
            let virt = TEMP_MAP_BASE + index * super::PAGE_SIZE;
            
            // TODO: Créer le mapping dans la table des pages
            // Pour l'instant, on retourne juste l'adresse calculée
            Ok(VirtualAddress::new(virt))
        }
    }
    
    /// Démappe une adresse temporaire
    pub fn unmap_temporary(_virt: VirtualAddress) {
        // Les mappings identity ne sont pas démappés
        // Les mappings temporaires seront réutilisés automatiquement
    }
    
    /// Active la pagination (déjà fait par boot.asm)
    pub fn enable_paging() {
        // La pagination est déjà activée par boot.asm
        // On pourrait recharger CR3 ici si nécessaire
    }
    
    /// Obtient l'adresse physique de la racine des tables de pages (CR3)
    pub fn get_page_table_root() -> PhysicalAddress {
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        }
        PhysicalAddress::new((cr3 & 0x000F_FFFF_FFFF_F000) as usize)
    }
    
    /// Définit l'adresse physique de la racine des tables de pages
    pub fn set_page_table_root(addr: PhysicalAddress) {
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) addr.value() as u64, options(nostack));
        }
    }
    
    /// Invalide l'entrée TLB pour une adresse virtuelle spécifique
    #[inline(always)]
    pub fn invalidate_tlb(addr: VirtualAddress) {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) addr.value(), options(nostack));
        }
    }
    
    /// Invalide tout le TLB (recharge CR3)
    #[inline(always)]
    pub fn invalidate_tlb_all() {
        unsafe {
            core::arch::asm!(
                "mov {tmp}, cr3",
                "mov cr3, {tmp}",
                tmp = out(reg) _,
                options(nostack)
            );
        }
    }
    
    /// Invalide une plage d'adresses TLB (4K pages)
    #[inline(always)]
    pub fn invalidate_tlb_range(start: VirtualAddress, num_pages: usize) {
        // Si plus de 64 pages, full flush est plus efficace
        if num_pages > 64 {
            invalidate_tlb_all();
            return;
        }
        
        let mut addr = start.value();
        for _ in 0..num_pages {
            unsafe {
                core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack));
            }
            addr += crate::arch::PAGE_SIZE;
        }
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
