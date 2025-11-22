//! # Mapper Mémoire Virtuelle → Physique
//! 
//! Ce module fournit une interface de plus haut niveau pour mapper et démapper
//! des plages de mémoire virtuelle, en s'occupant des détails de bas niveau
//! des tables de pages.

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError};
use super::PageTableFlags;
use crate::arch;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Mapper mémoire virtuelle
pub struct MemoryMapper {
    /// Navigateur dans les tables de pages
    walker: super::page_table::PageTableWalker,
    /// Statistiques du mapper
    stats: MapperStats,
}

/// Statistiques du mapper
#[derive(Debug)]
pub struct MapperStats {
    /// Nombre de pages mappées
    pub mapped_pages: AtomicUsize,
    /// Nombre de pages démapées
    pub unmapped_pages: AtomicUsize,
    /// Nombre de protections changées
    pub protection_changes: AtomicUsize,
}

impl MapperStats {
    /// Crée de nouvelles statistiques
    pub const fn new() -> Self {
        Self {
            mapped_pages: AtomicUsize::new(0),
            unmapped_pages: AtomicUsize::new(0),
            protection_changes: AtomicUsize::new(0),
        }
    }
    
    /// Incrémente le compteur de pages mappées
    pub fn inc_mapped_pages(&self) {
        self.mapped_pages.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de pages démapées
    pub fn inc_unmapped_pages(&self) {
        self.unmapped_pages.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de changements de protection
    pub fn inc_protection_changes(&self) {
        self.protection_changes.fetch_add(1, Ordering::Relaxed);
    }
}

impl MemoryMapper {
    /// Crée un nouveau mapper pour l'espace d'adressage actuel
    pub fn for_current_address_space() -> MemoryResult<Self> {
        let root_address = arch::mmu::get_page_table_root();
        let walker = super::page_table::PageTableWalker::new(root_address);
        
        Ok(Self {
            walker,
            stats: MapperStats::new(),
        })
    }
    
    /// Crée un nouveau mapper pour un espace d'adressage spécifique
    pub fn for_address_space(root_address: PhysicalAddress) -> MemoryResult<Self> {
        let walker = super::page_table::PageTableWalker::new(root_address);
        
        Ok(Self {
            walker,
            stats: MapperStats::new(),
        })
    }
    
    /// Mappe une page virtuelle à une page physique
    pub fn map_page(
        &mut self,
        virtual_addr: VirtualAddress,
        physical_addr: PhysicalAddress,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !virtual_addr.is_page_aligned() || !physical_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Mapper la page
        self.walker.map(virtual_addr, physical_addr, flags)?;
        
        // Invalider l'entrée TLB pour cette adresse
        arch::mmu::invalidate_tlb(virtual_addr);
        
        // Mettre à jour les statistiques
        self.stats.inc_mapped_pages();
        
        Ok(())
    }
    
    /// Démappe une page virtuelle
    pub fn unmap_page(&mut self, virtual_addr: VirtualAddress) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !virtual_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Démapper la page
        self.walker.unmap(virtual_addr)?;
        
        // Invalider l'entrée TLB pour cette adresse
        arch::mmu::invalidate_tlb(virtual_addr);
        
        // Mettre à jour les statistiques
        self.stats.inc_unmapped_pages();
        
        Ok(())
    }
    
    /// Change les protections d'une page
    pub fn protect_page(
        &mut self,
        virtual_addr: VirtualAddress,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !virtual_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Changer les protections
        self.walker.protect(virtual_addr, flags)?;
        
        // Invalider l'entrée TLB pour cette adresse
        arch::mmu::invalidate_tlb(virtual_addr);
        
        // Mettre à jour les statistiques
        self.stats.inc_protection_changes();
        
        Ok(())
    }
    
    /// Mappe une plage de pages
    pub fn map_range(
        &mut self,
        start_addr: VirtualAddress,
        physical_addr: PhysicalAddress,
        size: usize,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !start_addr.is_page_aligned() || !physical_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Arrondir la taille à une page entière
        let page_size = arch::PAGE_SIZE;
        let aligned_size = (size + page_size - 1) & !(page_size - 1);
        
        // Mapper chaque page
        let mut current_virtual = start_addr;
        let mut current_physical = physical_addr;
        let end_virtual = VirtualAddress::new(start_addr.value() + aligned_size);
        
        while current_virtual.value() < end_virtual.value() {
            self.map_page(current_virtual, current_physical, flags)?;
            
            current_virtual = VirtualAddress::new(current_virtual.value() + page_size);
            current_physical = PhysicalAddress::new(current_physical.value() + page_size);
        }
        
        Ok(())
    }
    
    /// Démappe une plage de pages
    pub fn unmap_range(&mut self, start_addr: VirtualAddress, size: usize) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !start_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Arrondir la taille à une page entière
        let page_size = arch::PAGE_SIZE;
        let aligned_size = (size + page_size - 1) & !(page_size - 1);
        
        // Démapper chaque page
        let mut current_virtual = start_addr;
        let end_virtual = VirtualAddress::new(start_addr.value() + aligned_size);
        
        while current_virtual.value() < end_virtual.value() {
            self.unmap_page(current_virtual)?;
            current_virtual = VirtualAddress::new(current_virtual.value() + page_size);
        }
        
        Ok(())
    }
    
    /// Change les protections d'une plage de pages
    pub fn protect_range(
        &mut self,
        start_addr: VirtualAddress,
        size: usize,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        // Vérifier l'alignement
        if !start_addr.is_page_aligned() {
            return Err(MemoryError::AlignmentError);
        }
        
        // Arrondir la taille à une page entière
        let page_size = arch::PAGE_SIZE;
        let aligned_size = (size + page_size - 1) & !(page_size - 1);
        
        // Changer les protections de chaque page
        let mut current_virtual = start_addr;
        let end_virtual = VirtualAddress::new(start_addr.value() + aligned_size);
        
        while current_virtual.value() < end_virtual.value() {
            self.protect_page(current_virtual, flags)?;
            current_virtual = VirtualAddress::new(current_virtual.value() + page_size);
        }
        
        Ok(())
    }
    
    /// Obtient l'adresse physique correspondant à une adresse virtuelle
    pub fn get_physical_address(&self, virtual_addr: VirtualAddress) -> MemoryResult<Option<PhysicalAddress>> {
        match self.walker.walk(virtual_addr)? {
            super::page_table::PageTableWalkResult::Present(physical_addr, _) => Ok(Some(physical_addr)),
            _ => Ok(None),
        }
    }
    
    /// Vérifie si une page est présente
    pub fn is_page_present(&self, virtual_addr: VirtualAddress) -> MemoryResult<bool> {
        match self.walker.walk(virtual_addr)? {
            super::page_table::PageTableWalkResult::Present(_, _) => Ok(true),
            _ => Ok(false),
        }
    }
    
    /// Obtient les flags d'une page
    pub fn get_page_flags(&self, virtual_addr: VirtualAddress) -> MemoryResult<Option<PageTableFlags>> {
        match self.walker.walk(virtual_addr)? {
            super::page_table::PageTableWalkResult::Present(_, flags) => Ok(Some(flags)),
            _ => Ok(None),
        }
    }
    
    /// Vérifie si une page est Copy-on-Write
    pub fn is_page_cow(&self, virtual_addr: VirtualAddress) -> MemoryResult<bool> {
        match self.walker.walk(virtual_addr)? {
            super::page_table::PageTableWalkResult::Present(_, flags) => Ok(flags.is_cow()),
            _ => Ok(false),
        }
    }
    
    /// Marque une page comme Copy-on-Write
    pub fn set_cow(&mut self, virtual_addr: VirtualAddress) -> MemoryResult<()> {
        // Obtenir les flags actuels
        let flags = self.get_page_flags(virtual_addr)?;
        
        if let Some(mut flags) = flags {
            // Ajouter le flag COW et retirer le flag d'écriture
            flags = flags.cow();
            flags = PageTableFlags(flags.0 & !PageTableFlags::new().writable().0);
            
            // Appliquer les nouveaux flags
            self.protect_page(virtual_addr, flags)
        } else {
            Err(MemoryError::InvalidAddress)
        }
    }
    
    /// Retourne les statistiques du mapper
    pub fn stats(&self) -> &MapperStats {
        &self.stats
    }
}

/// Initialise le mapper
pub fn init() -> MemoryResult<()> {
    log::info!("Memory mapper initialized");
    Ok(())
}

/// Mappe une page virtuelle à une page physique dans l'espace d'adressage actuel
pub fn map_page(
    virtual_addr: VirtualAddress,
    physical_addr: PhysicalAddress,
    flags: PageTableFlags,
) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.map_page(virtual_addr, physical_addr, flags)
}

/// Démappe une page virtuelle dans l'espace d'adressage actuel
pub fn unmap_page(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.unmap_page(virtual_addr)
}

/// Change les protections d'une page dans l'espace d'adressage actuel
pub fn protect_page(
    virtual_addr: VirtualAddress,
    flags: PageTableFlags,
) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.protect_page(virtual_addr, flags)
}

/// Mappe une plage de pages dans l'espace d'adressage actuel
pub fn map_range(
    start_addr: VirtualAddress,
    physical_addr: PhysicalAddress,
    size: usize,
    flags: PageTableFlags,
) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.map_range(start_addr, physical_addr, size, flags)
}

/// Démappe une plage de pages dans l'espace d'adressage actuel
pub fn unmap_range(start_addr: VirtualAddress, size: usize) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.unmap_range(start_addr, size)
}

/// Change les protections d'une plage de pages dans l'espace d'adressage actuel
pub fn protect_range(
    start_addr: VirtualAddress,
    size: usize,
    flags: PageTableFlags,
) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.protect_range(start_addr, size, flags)
}

/// Obtient l'adresse physique correspondant à une adresse virtuelle dans l'espace d'adressage actuel
pub fn get_physical_address(virtual_addr: VirtualAddress) -> MemoryResult<Option<PhysicalAddress>> {
    let mapper = MemoryMapper::for_current_address_space()?;
    mapper.get_physical_address(virtual_addr)
}

/// Vérifie si une page est présente dans l'espace d'adressage actuel
pub fn is_page_present(virtual_addr: VirtualAddress) -> MemoryResult<bool> {
    let mapper = MemoryMapper::for_current_address_space()?;
    mapper.is_page_present(virtual_addr)
}

/// Vérifie si une page est Copy-on-Write dans l'espace d'adressage actuel
pub fn is_page_cow(virtual_addr: VirtualAddress) -> MemoryResult<bool> {
    let mapper = MemoryMapper::for_current_address_space()?;
    mapper.is_page_cow(virtual_addr)
}

/// Marque une page comme Copy-on-Write dans l'espace d'adressage actuel
pub fn set_cow(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    let mut mapper = MemoryMapper::for_current_address_space()?;
    mapper.set_cow(virtual_addr)
}
