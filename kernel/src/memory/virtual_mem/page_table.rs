//! # Opérations sur les Tables de Pages
//! 
//! Ce module implémente les opérations de bas niveau sur les tables de pages,
//! incluant la création, la navigation et la manipulation des entrées.

extern crate alloc;
use alloc::vec::Vec;

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError};
use crate::arch;

// Re-export PageTableFlags from parent module
pub use super::PageTableFlags;

/// Nombre de niveaux dans la hiérarchie des tables de pages (x86_64: 4)
pub const PAGE_TABLE_LEVELS: usize = 4;

/// Nombre d'entrées par table de pages (512 pour x86_64)
pub const PAGE_TABLE_ENTRIES: usize = 512;

/// Taille d'une table de pages en octets
pub const PAGE_TABLE_SIZE: usize = PAGE_TABLE_ENTRIES * core::mem::size_of::<PageTableEntry>();

/// Représente une entrée dans une table de pages
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Crée une nouvelle entrée (non présente)
    pub const fn new() -> Self {
        Self(0)
    }
    
    /// Crée une entrée pointant vers une table de pages ou une page
    pub fn new_frame(address: PhysicalAddress, flags: PageTableFlags) -> Self {
        Self(address.value() as u64 | flags.0)
    }
    
    /// Retourne l'adresse physique de la table ou de la page
    pub fn address(&self) -> PhysicalAddress {
        PhysicalAddress::new((self.0 & 0x000FFFFFFFFFF000) as usize)
    }
    
    /// Retourne les flags de l'entrée
    pub fn flags(&self) -> PageTableFlags {
        PageTableFlags(self.0 & 0x8000000000000FFF)
    }
    
    /// Définit l'adresse physique
    pub fn set_address(&mut self, address: PhysicalAddress) {
        self.0 = (self.0 & 0x8000000000000FFF) | (address.value() as u64);
    }
    
    /// Définit les flags
    pub fn set_flags(&mut self, flags: PageTableFlags) {
        self.0 = (self.0 & 0x000FFFFFFFFFF000) | flags.0;
    }
    
    /// Vérifie si l'entrée est présente
    pub fn is_present(&self) -> bool {
        self.flags().is_present()
    }
    
    /// Vérifie si l'entrée pointe vers une page de grande taille
    pub fn is_huge(&self) -> bool {
        self.flags().0 & 0x80 != 0
    }
}

/// Représente une table de pages
#[derive(Debug)]
pub struct PageTable {
    /// Adresse physique de cette table de pages
    physical_address: PhysicalAddress,
    /// Adresse virtuelle de cette table de pages (pour y accéder)
    virtual_address: VirtualAddress,
    /// Niveau de cette table dans la hiérarchie (0 = PML4, 3 = PT)
    level: usize,
}

impl PageTable {
    /// Crée une nouvelle table de pages
    pub fn new(level: usize) -> MemoryResult<Self> {
        // Allouer une frame physique pour la table
        let frame = crate::memory::physical::allocate_frame()?;
        
        // Mapper cette frame dans l'espace d'adressage du noyau
        let virtual_address = VirtualAddress::from(arch::mmu::map_temporary(frame.address())?);
        
        // Initialiser la table (zéro)
        unsafe {
            core::ptr::write_bytes(virtual_address.value() as *mut u8, 0, arch::PAGE_SIZE);
        }
        
        Ok(Self {
            physical_address: frame.address(),
            virtual_address,
            level,
        })
    }
    
    /// Crée une table de pages à partir d'une adresse physique existante
    pub fn from_physical(physical_address: PhysicalAddress, level: usize) -> MemoryResult<Self> {
        // Mapper cette frame dans l'espace d'adressage du noyau
        let virtual_address = VirtualAddress::from(arch::mmu::map_temporary(physical_address)?);
        
        Ok(Self {
            physical_address,
            virtual_address,
            level,
        })
    }
    
    /// Retourne l'adresse physique de cette table
    pub fn physical_address(&self) -> PhysicalAddress {
        self.physical_address
    }
    
    /// Retourne une référence à une entrée
    pub fn entry(&self, index: usize) -> MemoryResult<PageTableEntry> {
        if index >= PAGE_TABLE_ENTRIES {
            return Err(MemoryError::InvalidAddress);
        }
        
        unsafe {
            let entries = self.virtual_address.value() as *const PageTableEntry;
            Ok(*entries.add(index))
        }
    }
    
    /// Retourne une référence mutable à une entrée
    pub fn entry_mut(&mut self, index: usize) -> MemoryResult<&mut PageTableEntry> {
        if index >= PAGE_TABLE_ENTRIES {
            return Err(MemoryError::InvalidAddress);
        }
        
        unsafe {
            let entries = self.virtual_address.value() as *mut PageTableEntry;
            Ok(&mut *entries.add(index))
        }
    }
    
    /// Itère sur les entrées de cette table
    pub fn entries(&self) -> impl Iterator<Item = (usize, PageTableEntry)> + '_ {
        (0..PAGE_TABLE_ENTRIES).filter_map(move |i| {
            self.entry(i).ok().map(|entry| (i, entry))
        })
    }
    
    /// Itère sur les entrées présentes de cette table
    pub fn present_entries(&self) -> impl Iterator<Item = (usize, PageTableEntry)> + '_ {
        self.entries().filter(|(_, entry)| entry.is_present())
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        // Démapper la table de l'espace d'adressage du noyau
        arch::mmu::unmap_temporary(self.virtual_address);
        
        // Libérer la frame physique
        let frame = crate::memory::physical::Frame::containing_address(self.physical_address);
        let _ = crate::memory::physical::deallocate_frame(frame);
    }
}

/// Résultat de la navigation dans les tables de pages
#[derive(Debug)]
pub enum PageTableWalkResult {
    /// La page est présente
    Present(PhysicalAddress, PageTableFlags),
    /// La page n'est pas présente
    NotPresent,
    /// Une entrée dans la hiérarchie n'est pas présente
    HierarchicalNotPresent(usize),
    /// L'adresse est invalide
    InvalidAddress,
}

/// Navigateur dans les tables de pages
pub struct PageTableWalker {
    /// Racine de la hiérarchie des tables de pages (adresse physique du PML4)
    root_address: PhysicalAddress,
}

impl PageTableWalker {
    /// Crée un nouveau navigateur
    pub fn new(root_address: PhysicalAddress) -> Self {
        Self { root_address }
    }
    
    /// Navigue jusqu'à une adresse virtuelle
    pub fn walk(&self, virtual_addr: VirtualAddress) -> MemoryResult<PageTableWalkResult> {
        let mut current_address = self.root_address;
        
        for level in (0..PAGE_TABLE_LEVELS).rev() {
            let table = PageTable::from_physical(current_address, level)?;
            
            let index = self.get_index(virtual_addr, level);
            let entry = table.entry(index)?;
            
            if !entry.is_present() {
                if level == 0 {
                    return Ok(PageTableWalkResult::NotPresent);
                } else {
                    return Ok(PageTableWalkResult::HierarchicalNotPresent(level));
                }
            }
            
            if level == 0 || entry.is_huge() {
                // On a trouvé la page finale
                return Ok(PageTableWalkResult::Present(entry.address(), entry.flags()));
            }
            
            // Passer à la table de pages suivante
            current_address = entry.address();
        }
        
        Err(MemoryError::InternalError("Page table walk failed"))
    }
    
    /// Mappe une page virtuelle à une page physique
    pub fn map(
        &mut self,
        virtual_addr: VirtualAddress,
        physical_addr: PhysicalAddress,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        let mut current_address = self.root_address;
        
        for level in (0..PAGE_TABLE_LEVELS).rev() {
            let mut table = PageTable::from_physical(current_address, level)?;
            
            let index = self.get_index(virtual_addr, level);
            let entry = table.entry_mut(index)?;
            
            if level == 0 {
                // Niveau final, mapper la page
                *entry = PageTableEntry::new_frame(physical_addr, flags);
                return Ok(());
            }
            
            if !entry.is_present() {
                // Allouer une nouvelle table de pages
                let new_table = PageTable::new(level - 1)?;
                *entry = PageTableEntry::new_frame(
                    new_table.physical_address(),
                    PageTableFlags::new().present().writable().user(),
                );
                
                // La nouvelle table sera libérée quand elle n'est plus nécessaire
                core::mem::forget(new_table);
            } else if entry.is_huge() {
                return Err(MemoryError::InternalError("Cannot map inside huge page"));
            }
            
            current_address = entry.address();
        }
        
        Err(MemoryError::InternalError("Page table mapping failed"))
    }
    
    /// Démappe une page virtuelle
    pub fn unmap(&mut self, virtual_addr: VirtualAddress) -> MemoryResult<()> {
        let mut current_address = self.root_address;
        let mut tables_to_free = Vec::new();
        
        for level in (0..PAGE_TABLE_LEVELS).rev() {
            let mut table = PageTable::from_physical(current_address, level)?;
            
            let index = self.get_index(virtual_addr, level);
            let entry = table.entry_mut(index)?;
            
            if !entry.is_present() {
                return Err(MemoryError::InvalidAddress);
            }
            
            if level == 0 {
                // Niveau final, démapper la page
                *entry = PageTableEntry::new();
                break;
            }
            
            current_address = entry.address();
            tables_to_free.push((table.physical_address(), index));
        }
        
        // Vérifier si les tables de pages parents peuvent être libérées
        for (table_address, _entry_index) in tables_to_free.iter().rev() {
            let table = PageTable::from_physical(*table_address, 0)?;
            
            // Vérifier si toutes les entrées sont non présentes
            let mut all_empty = true;
            for entry in table.entries() {
                if entry.1.is_present() {
                    all_empty = false;
                    break;
                }
            }
            
            if all_empty {
                // Libérer la table de pages
                let frame = crate::memory::physical::Frame::containing_address(*table_address);
                crate::memory::physical::deallocate_frame(frame);
            }
        }
        
        Ok(())
    }
    
    /// Change les flags d'une page
    pub fn protect(
        &mut self,
        virtual_addr: VirtualAddress,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        let mut current_address = self.root_address;
        
        for level in (0..PAGE_TABLE_LEVELS).rev() {
            let table = PageTable::from_physical(current_address, level)?;
            
            let index = self.get_index(virtual_addr, level);
            let entry = table.entry(index)?;
            
            if !entry.is_present() {
                return Err(MemoryError::InvalidAddress);
            }
            
            if level == 0 || entry.is_huge() {
                // Niveau final, changer les flags
                let mut table = PageTable::from_physical(current_address, level)?;
                let entry_mut = table.entry_mut(index)?;
                entry_mut.set_flags(flags);
                return Ok(());
            }
            
            current_address = entry.address();
        }
        
        Err(MemoryError::InternalError("Page table protection failed"))
    }
    
    /// Obtient l'index dans une table de pages pour une adresse et un niveau donnés
    fn get_index(&self, virtual_addr: VirtualAddress, level: usize) -> usize {
        let shift = 12 + level * 9; // 12 bits pour l'offset, 9 bits par niveau
        (virtual_addr.value() >> shift) & 0x1FF
    }
}

/// Initialise les tables de pages du noyau
pub fn init() -> MemoryResult<()> {
    // Créer la table de pages racine (PML4)
    let root_table = PageTable::new(PAGE_TABLE_LEVELS - 1)?;
    
    // Mapper le noyau
    let kernel_start = crate::arch::KERNEL_START_ADDRESS;
    let kernel_end = crate::arch::KERNEL_END_ADDRESS;
    let kernel_size = (kernel_end - kernel_start) as usize;
    
    let mut walker = PageTableWalker::new(root_table.physical_address());
    
    // Mapper le code et les données du noyau
    for offset in (0..kernel_size).step_by(arch::PAGE_SIZE) {
        let virtual_addr = VirtualAddress::new((kernel_start as usize) + offset);
        let physical_addr = PhysicalAddress::new(virtual_addr.value() - (crate::arch::KERNEL_VIRTUAL_OFFSET as usize));
        
        let flags = PageTableFlags::new()
            .present()
            .writable()
            .global()
            .no_execute(); // Le code du noyau est marqué NX par défaut pour la sécurité
        
        // Si c'est dans la région du code du noyau, autoriser l'exécution
        let flags = if virtual_addr.value() >= (crate::arch::KERNEL_CODE_START as usize)
            && virtual_addr.value() < (crate::arch::KERNEL_CODE_END as usize) {
            PageTableFlags::new()
                .present()
                .writable()
                .global()
                .execute()
        } else {
            flags
        };
        
        walker.map(virtual_addr, physical_addr, flags)?;
    }
    
    // Activer la pagination
    arch::mmu::enable_paging();
    
    log::info!("Kernel page tables initialized");
    Ok(())
}
