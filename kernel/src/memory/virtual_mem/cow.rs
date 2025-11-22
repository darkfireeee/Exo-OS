//! # Implémentation du Copy-on-Write (CoW)
//! 
//! Ce module implémente le mécanisme Copy-on-Write, qui permet de partager
//! des pages entre processus jusqu'à ce qu'une écriture soit nécessaire,
//! moment auquel une copie est faite.

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError};
use crate::arch;
use core::sync::atomic::{AtomicUsize, Ordering};
extern crate alloc;

/// Représente une page partagée avec Copy-on-Write
#[derive(Debug)]
pub struct CowPage {
    /// Adresse physique de la page partagée
    pub physical_address: PhysicalAddress,
    /// Nombre de références à cette page
    pub ref_count: AtomicUsize,
    /// Taille de la page
    pub size: usize,
}

impl CowPage {
    /// Crée une nouvelle page CoW
    pub fn new(physical_address: PhysicalAddress, size: usize) -> Self {
        Self {
            physical_address,
            ref_count: AtomicUsize::new(1),
            size,
        }
    }
    
    /// Incrémente le compteur de références
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Décrémente le compteur de références
    pub fn dec_ref(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::Relaxed) - 1
    }
    
    /// Retourne le nombre de références actuel
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Relaxed)
    }
}

/// Gestionnaire de pages Copy-on-Write
pub struct CowManager {
    /// Liste des pages CoW
    pages: spin::Mutex<alloc::collections::BTreeMap<PhysicalAddress, CowPage>>,
    /// Statistiques du gestionnaire CoW
    stats: CowStats,
}

/// Statistiques du gestionnaire CoW
#[derive(Debug)]
pub struct CowStats {
    /// Nombre de pages CoW créées
    pub created_pages: AtomicUsize,
    /// Nombre de copies effectuées
    pub copies_performed: AtomicUsize,
    /// Nombre de fautes de page CoW gérées
    pub cow_faults_handled: AtomicUsize,
    /// Nombre de pages libérées
    pub freed_pages: AtomicUsize,
}

impl CowStats {
    /// Crée de nouvelles statistiques
    pub const fn new() -> Self {
        Self {
            created_pages: AtomicUsize::new(0),
            copies_performed: AtomicUsize::new(0),
            cow_faults_handled: AtomicUsize::new(0),
            freed_pages: AtomicUsize::new(0),
        }
    }
    
    /// Incrémente le compteur de pages créées
    pub fn inc_created_pages(&self) {
        self.created_pages.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de copies effectuées
    pub fn inc_copies_performed(&self) {
        self.copies_performed.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de fautes de page CoW gérées
    pub fn inc_cow_faults_handled(&self) {
        self.cow_faults_handled.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de pages libérées
    pub fn inc_freed_pages(&self) {
        self.freed_pages.fetch_add(1, Ordering::Relaxed);
    }
}

impl CowManager {
    /// Crée un nouveau gestionnaire CoW
    pub const fn new() -> Self {
        Self {
            pages: spin::Mutex::new(alloc::collections::BTreeMap::new()),
            stats: CowStats::new(),
        }
    }
    
    /// Crée une nouvelle page CoW
    pub fn create_cow_page(&self, physical_address: PhysicalAddress, size: usize) -> MemoryResult<()> {
        let mut pages = self.pages.lock();
        
        if pages.contains_key(&physical_address) {
            return Err(MemoryError::InvalidAddress);
        }
        
        let cow_page = CowPage::new(physical_address, size);
        pages.insert(physical_address, cow_page);
        
        self.stats.inc_created_pages();
        
        Ok(())
    }
    
    /// Partage une page CoW
    pub fn share_cow_page(&self, physical_address: PhysicalAddress) -> MemoryResult<()> {
        let mut pages = self.pages.lock();
        
        if let Some(cow_page) = pages.get_mut(&physical_address) {
            cow_page.inc_ref();
            Ok(())
        } else {
            Err(MemoryError::InvalidAddress)
        }
    }
    
    /// Gère une faute de page CoW
    pub fn handle_cow_fault(&self, virtual_addr: VirtualAddress) -> MemoryResult<()> {
        // Obtenir l'adresse physique actuelle
        let current_physical = super::mapper::get_physical_address(virtual_addr)?
            .ok_or(MemoryError::InvalidAddress)?;
        
        // Vérifier si c'est une page CoW
        {
            let pages = self.pages.lock();
            if let Some(cow_page) = pages.get(&current_physical) {
                // Vérifier le compteur de références
                if cow_page.ref_count() == 1 {
                    // Nous sommes le seul propriétaire, il suffit de rendre la page inscriptible
                    let mut mapper = super::mapper::MemoryMapper::for_current_address_space()?;
                    let mut flags = mapper.get_page_flags(virtual_addr)?
                        .ok_or(MemoryError::InvalidAddress)?;
                    
                    // Retirer le flag CoW et ajouter le flag d'écriture
                    flags = super::page_table::PageTableFlags(flags.0 & !super::page_table::PageTableFlags::new().cow().0);
                    flags = flags.writable();
                    
                    mapper.protect_page(virtual_addr, flags)?;
                    
                    self.stats.inc_cow_faults_handled();
                    return Ok(());
                }
            } else {
                // Ce n'est pas une page CoW
                return Err(MemoryError::InvalidAddress);
            }
        }
        
        // Allouer une nouvelle page physique
        let new_frame = crate::memory::physical::allocate_frame()?;
        let new_physical = new_frame.address();
        
        // Mapper la nouvelle page temporairement
        let temp_virtual = arch::mmu::map_temporary(new_physical)?;
        
        // Copier le contenu de l'ancienne page vers la nouvelle
        let old_virtual = arch::mmu::map_temporary(current_physical)?;
        
        unsafe {
            core::ptr::copy_nonoverlapping(
                old_virtual.value() as *const u8,
                temp_virtual.value() as *mut u8,
                arch::PAGE_SIZE,
            );
        }
        
        // Démapper les pages temporaires
        let _ = arch::mmu::unmap_temporary(old_virtual);
        let _ = arch::mmu::unmap_temporary(temp_virtual);
        
        // Mapper la nouvelle page à l'adresse virtuelle
        let mut mapper = super::mapper::MemoryMapper::for_current_address_space()?;
        let flags = super::page_table::PageTableFlags::new()
            .present()
            .writable()
            .user();
        
        mapper.map_page(virtual_addr, new_physical, flags)?;
        
        // Décrémenter le compteur de références de l'ancienne page
        {
            let mut pages = self.pages.lock();
            if let Some(cow_page) = pages.get_mut(&current_physical) {
                if cow_page.dec_ref() == 0 {
                    // Plus personne n'utilise cette page, la libérer
                    let frame = crate::memory::physical::Frame::containing_address(current_physical);
                    crate::memory::physical::deallocate_frame(frame)?;
                    
                    pages.remove(&current_physical);
                    self.stats.inc_freed_pages();
                }
            }
        }
        
        self.stats.inc_copies_performed();
        self.stats.inc_cow_faults_handled();
        
        Ok(())
    }
    
    /// Libère une page CoW
    pub fn free_cow_page(&self, physical_address: PhysicalAddress) -> MemoryResult<()> {
        let mut pages = self.pages.lock();
        
        if let Some(cow_page) = pages.get_mut(&physical_address) {
            if cow_page.dec_ref() == 0 {
                // Plus personne n'utilise cette page, la libérer
                let frame = crate::memory::physical::Frame::containing_address(physical_address);
                crate::memory::physical::deallocate_frame(frame)?;
                
                pages.remove(&physical_address);
                self.stats.inc_freed_pages();
            }
            
            Ok(())
        } else {
            Err(MemoryError::InvalidAddress)
        }
    }
    
    /// Retourne les statistiques du gestionnaire CoW
    pub fn stats(&self) -> &CowStats {
        &self.stats
    }
}

/// Gestionnaire global CoW
static mut COW_MANAGER: CowManager = CowManager::new();
static mut COW_MANAGER_INITIALIZED: bool = false;

/// Initialise le gestionnaire CoW
pub fn init() -> MemoryResult<()> {
    unsafe {
        if COW_MANAGER_INITIALIZED {
            return Ok(());
        }
        
        COW_MANAGER_INITIALIZED = true;
        
        Ok(())
    }
}

/// Retourne le gestionnaire CoW
pub fn get_manager() -> &'static CowManager {
    unsafe {
        assert!(COW_MANAGER_INITIALIZED, "COW manager not initialized");
        &COW_MANAGER
    }
}

/// Crée une nouvelle page CoW
pub fn create_cow_page(physical_address: PhysicalAddress, size: usize) -> MemoryResult<()> {
    get_manager().create_cow_page(physical_address, size)
}

/// Partage une page CoW
pub fn share_cow_page(physical_address: PhysicalAddress) -> MemoryResult<()> {
    get_manager().share_cow_page(physical_address)
}

/// Gère une faute de page CoW
pub fn handle_cow_fault(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    get_manager().handle_cow_fault(virtual_addr)
}

/// Libère une page CoW
pub fn free_cow_page(physical_address: PhysicalAddress) -> MemoryResult<()> {
    get_manager().free_cow_page(physical_address)
}

/// Retourne les statistiques du gestionnaire CoW
pub fn get_stats() -> &'static CowStats {
    get_manager().stats()
}
