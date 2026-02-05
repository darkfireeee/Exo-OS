//! # Opérations sur les Tables de Pages
//! 
//! Ce module implémente les opérations de bas niveau sur les tables de pages,
//! incluant la création, la navigation et la manipulation des entrées.

extern crate alloc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError};
use crate::arch;
use spin::Mutex;

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
    /// Si true, cette PageTable possède la frame et doit la libérer au Drop
    /// Si false, c'est juste une référence à une table existante
    owns_frame: bool,
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
            owns_frame: true,  // new() crée et possède la frame
        })
    }
    
    /// Crée une table de pages à partir d'une adresse physique existante
    pub fn from_physical(physical_address: PhysicalAddress, level: usize) -> MemoryResult<Self> {
        // Mapper cette frame dans l'espace d'adressage du noyau
        crate::logger::early_print("[from_physical] Calling map_temporary\n");
        let virtual_address = VirtualAddress::from(arch::mmu::map_temporary(physical_address)?);
        crate::logger::early_print("[from_physical] map_temporary returned\n");
        
        crate::logger::early_print("[from_physical] Creating result struct\n");
        let result = Self {
            physical_address,
            virtual_address,
            level,
            owns_frame: false,  // from_physical() ne possède PAS la frame
        };
        crate::logger::early_print("[from_physical] Struct created, returning Ok\n");
        Ok(result)
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
        crate::logger::early_print("[PageTable::drop] START\n");
        // Démap per la table de l'espace d'adressage du noyau
        arch::mmu::unmap_temporary(self.virtual_address);
        crate::logger::early_print("[PageTable::drop] unmap done\n");
        
        // Libérer la frame physique SEULEMENT si cette PageTable la possède
        if self.owns_frame {
            crate::logger::early_print("[PageTable::drop] Deallocating owned frame\n");
            let frame = crate::memory::physical::Frame::containing_address(self.physical_address);
            let _ = crate::memory::physical::deallocate_frame(frame);
        } else {
            crate::logger::early_print("[PageTable::drop] Not deallocating (not owned)\n");
        }
        crate::logger::early_print("[PageTable::drop] END\n");
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
    /// Cache des page tables créées lors de splits de huge pages
    /// Clé: adresse virtuelle de base de la huge page (2MB-aligned)
    /// Valeur: adresse physique de la PT créée
    split_cache: Mutex<BTreeMap<usize, PhysicalAddress>>,
}

impl PageTableWalker {
    /// Crée un nouveau navigateur
    pub fn new(root_address: PhysicalAddress) -> Self {
        Self { 
            root_address,
            split_cache: Mutex::new(BTreeMap::new()),
        }
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
    
    /// Split a huge page (2MB) into 512 normal pages (4KB each)
    /// 
    /// This is called when we need to map a 4KB page inside an existing 2MB huge page.
    /// The huge page is split transparently, preserving the existing mappings.
    ///
    /// OPTIMIZATION: Uses a cache to avoid re-splitting the same huge page multiple times.
    /// If the huge page at this virtual address was already split, return the cached PT.
    ///
    /// # Arguments
    /// * `level` - Current page table level (should be 1 for 2MB huge pages in PD)
    /// * `huge_entry` - The huge page entry to split
    /// * `virtual_base` - Virtual address of the huge page base
    ///
    /// # Returns
    /// A new PageTable (level 0 = PT) containing 512 entries mapping the same physical memory
    fn split_huge_page(
        &mut self,
        level: usize,
        huge_entry: &PageTableEntry,
        virtual_base: VirtualAddress,
    ) -> MemoryResult<PageTable> {
        crate::logger::early_print("[SPLIT] Entered split_huge_page()\n");
        
        // Validate: must be at level 1 (PD - Page Directory) with huge flag
        // x86-64 hierarchy: PML4(3) -> PDPT(2) -> PD(1) -> PT(0)
        // 2MB huge pages are in PD entries (level 1)
        if level != 1 {
            log::error!(
                "[MMU] Invalid split level: {} (expected 1 for 2MB pages)",
                level
            );
            return Err(MemoryError::InternalError("Can only split 2MB huge pages at level 1 (PD)"));
        }
        
        if !huge_entry.is_huge() {
            return Err(MemoryError::InternalError("Entry is not a huge page"));
        }
        
        // OPTIMIZATION 1: Check split cache first (avoid re-splitting)
        let cache_key = virtual_base.value();
        {
            let cache = self.split_cache.lock();
            if let Some(&cached_pt_phys) = cache.get(&cache_key) {
                log::debug!(
                    "[MMU] Cache hit: Huge page at {:#x} was already split, reusing PT at {:#x}",
                    virtual_base.value(),
                    cached_pt_phys.value()
                );
                return PageTable::from_physical(cached_pt_phys, 0);
            }
        }
        
        // NO LOGGING IN CRITICAL SECTION TO AVOID DEADLOCK
        // See PAGE_SPLITTING_DESIGN.md section 4: TLB Flush Investigation
        // Logging can deadlock because logger needs memory operations
        
        // Extract base physical address and flags from huge page
        let huge_phys_base = huge_entry.address();
        let huge_flags = huge_entry.flags();
        
        // Allocate PT frame directly without recursive mapping
        let pt_frame = crate::memory::physical::allocate_frame()?;
        let pt_phys = pt_frame.address();
        
        // Map the PT frame temporarily to initialize it
        let pt_virt = VirtualAddress::from(arch::mmu::map_temporary(pt_phys)?);
        
        // Zero-initialize the PT
        unsafe {
            core::ptr::write_bytes(pt_virt.value() as *mut u8, 0, arch::PAGE_SIZE);
        }
        
        // Populate all 512 entries directly via the temporary mapping
        unsafe {
            let entries = pt_virt.value() as *mut PageTableEntry;
            
            for i in 0..PAGE_TABLE_ENTRIES {
                let phys_addr = PhysicalAddress::new(
                    huge_phys_base.value() + i * arch::PAGE_SIZE
                );
                
                // Preserve flags from huge page (except huge bit)
                let mut page_flags = huge_flags;
                page_flags.0 &= !0x80; // Clear huge bit (bit 7)
                
                // Write entry directly
                let entry = PageTableEntry::new_frame(phys_addr, page_flags);
                *entries.add(i) = entry;
            }
        }
        
        // Unmap the temporary mapping - we're done initializing
        arch::mmu::unmap_temporary(pt_virt);
        
        // Create a PageTable structure to return
        let split_pt = PageTable::from_physical(pt_phys, 0)?;
        
        // OPTIMIZATION 2: Add to split cache before TLB flush
        {
            let mut cache = self.split_cache.lock();
            cache.insert(cache_key, pt_phys);
        }
        
        // TLB flush: Use flush_all() which is faster than 512 individual INVLPGs
        crate::arch::x86_64::memory::tlb::flush_all();
        
        // End of critical section - safe to log now
        crate::logger::early_print("[SPLIT] About to return\n");
        log::info!("[MMU] Split complete: {:#x} → 512×4KB (cached)",
            virtual_base.value()
        );
        crate::logger::early_print("[SPLIT] Returning OK\n");
        
        Ok(split_pt)
    }
    
    /// Mappe une page virtuelle à une page physique
    pub fn map(
        &mut self,
        virtual_addr: VirtualAddress,
        physical_addr: PhysicalAddress,
        flags: PageTableFlags,
    ) -> MemoryResult<()> {
        crate::logger::early_print("[map()] START\n");
        let mut current_address = self.root_address;
        crate::logger::early_print("[map()] Got root\n");
        
        for level in (0..PAGE_TABLE_LEVELS).rev() {
            crate::logger::early_print("[map()] Creating table from_physical\n");
            let table_result = PageTable::from_physical(current_address, level);
            crate::logger::early_print("[map()] from_physical call completed\n");
            
            crate::logger::early_print("[map()] Checking if Ok...\n");
            if table_result.is_ok() {
                crate::logger::early_print("[map()] Result is Ok!\n");
            } else {
                crate::logger::early_print("[map()] Result is Err!\n");
            }
            
            crate::logger::early_print("[map()] About to extract table from Result\n");
            let mut table = table_result?;
            crate::logger::early_print("[map()] Unwrap completed!\n");
            crate::logger::early_print("[map()] Table value received!\n");
            crate::logger::early_print("[map()] Table extracted successfully\n");
            
            crate::logger::early_print("[map()] Getting index\n");
            let index = self.get_index(virtual_addr, level);
            crate::logger::early_print("[map()] Index obtained\n");
            
            crate::logger::early_print("[map()] Getting entry_mut\n");
            let entry = table.entry_mut(index)?;
            crate::logger::early_print("[map()] Entry obtained\n");
            
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
                
                // La nouvelle table sera libérée qu elle n'est plus nécessaire
                core::mem::forget(new_table);
            } else if entry.is_huge() {
                // Split the huge page into 512 normal pages
                // Calculate the base virtual address of the huge page
                // For level 2 (PDE), each entry covers 2MB = 512 * 4KB
                crate::logger::early_print("[MAP] Computing huge_page_size...\n");
                let huge_page_size = arch::PAGE_SIZE * PAGE_TABLE_ENTRIES; // 2MB
                crate::logger::early_print("[MAP] Computing virtual_base...\n");
                let virtual_base = VirtualAddress::new(
                    (virtual_addr.value() / huge_page_size) * huge_page_size
                );
                
                // NO LOGGING HERE - can cause deadlock
                // See PAGE_SPLITTING_DESIGN.md section 4
                
                // Perform the split
                crate::logger::early_print("[MAP] Calling split_huge_page()...\n");
                let new_table = self.split_huge_page(level, entry, virtual_base)?;
                crate::logger::early_print("[MAP] split_huge_page() returned\n");
                
                // Replace the huge page entry with a pointer to the new PT
                *entry = PageTableEntry::new_frame(
                    new_table.physical_address(),
                    PageTableFlags::new().present().writable().user(),
                );
                
                // Don't drop the new table - it's now part of the page table hierarchy
                core::mem::forget(new_table);
                
                // NO LOGGING HERE - can cause deadlock
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
    
    /// Retourne le nombre d'entrées dans le cache de splits
    /// 
    /// Utile pour les statistiques et le débogage.
    pub fn split_cache_size(&self) -> usize {
        self.split_cache.lock().len()
    }
    
    /// Vide le cache de splits
    /// 
    /// ATTENTION: Cela ne libère PAS la mémoire physique des PTs cachées.
    /// À utiliser seulement lors d'un changement complet de contexte de page tables
    /// (par exemple, switch de processus).
    pub fn clear_split_cache(&mut self) {
        let count = self.split_cache.lock().len();
        self.split_cache.lock().clear();
        log::debug!("[MMU] Split cache cleared ({} entries removed)", count);
    }
    
    /// Vérifie si une huge page à cette adresse virtuelle a déjà été splitée
    /// 
    /// Retourne Some(physical_address) si elle existe dans le cache, None sinon.
    pub fn check_split_cache(&self, virtual_base: VirtualAddress) -> Option<PhysicalAddress> {
        self.split_cache.lock().get(&virtual_base.value()).copied()
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
