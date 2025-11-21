//! # Gestion de la Mémoire Virtuelle
//! 
//! Ce module est responsable de la gestion de la mémoire virtuelle, incluant
//! les tables de pages, la translation d'adresses et les espaces d'adressage
//! pour chaque processus. C'est la base de l'isolation des processus.

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError, PageProtection};
use crate::arch;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Types d'entrées dans les tables de pages
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageTableEntryType {
    /// Entrée non présente
    NotPresent,
    /// Entrée pour une page de 4KB
    Page4Kb,
    /// Entrée pour une page de 2MB (huge page)
    Page2Mb,
    /// Entrée pour une page de 1GB (gigantic page)
    Page1Gb,
}

/// Flags pour une entrée de table de pages
#[derive(Debug, Clone, Copy)]
pub struct PageTableFlags(u64);

impl PageTableFlags {
    /// Crée de nouveaux flags (tous désactivés)
    pub const fn new() -> Self {
        Self(0)
    }
    
    /// La page est présente en mémoire
    pub const fn present(self) -> Self {
        Self(self.0 | 0x1)
    }
    
    /// La page est inscriptible
    pub const fn writable(self) -> Self {
        Self(self.0 | 0x2)
    }
    
    /// La page est accessible en mode utilisateur
    pub const fn user(self) -> Self {
        Self(self.0 | 0x4)
    }
    
    /// Write-Through (cache)
    pub const fn write_through(self) -> Self {
        Self(self.0 | 0x8)
    }
    
    /// Cache-Disable
    pub const fn cache_disable(self) -> Self {
        Self(self.0 | 0x10)
    }
    
    /// La page a été accédée
    pub const fn accessed(self) -> Self {
        Self(self.0 | 0x20)
    }
    
    /// La page a été modifiée (dirty)
    pub const fn dirty(self) -> Self {
        Self(self.0 | 0x40)
    }
    
    /// Page de grande taille (2MB/1GB)
    pub const fn huge(self) -> Self {
        Self(self.0 | 0x80)
    }
    
    /// Global (ignorée dans TLB flush sur CR3 change)
    pub const fn global(self) -> Self {
        Self(self.0 | 0x100)
    }
    
    /// Copy-on-Write
    pub const fn cow(self) -> Self {
        Self(self.0 | 0x200)
    }
    
    /// No-Execute (NX bit)
    pub const fn no_execute(self) -> Self {
        Self(self.0 | (1u64 << 63))
    }
    
    /// Autorise l'exécution (désactive le bit NX)
    pub const fn execute(self) -> Self {
        Self(self.0 & !(1u64 << 63))
    }
    
    /// Vérifie si le flag présent est actif
    pub fn is_present(&self) -> bool {
        self.0 & 0x1 != 0
    }
    
    /// Vérifie si le flag inscriptible est actif
    pub fn is_writable(&self) -> bool {
        self.0 & 0x2 != 0
    }
    
    /// Vérifie si le flag utilisateur est actif
    pub fn is_user(&self) -> bool {
        self.0 & 0x4 != 0
    }
    
    /// Vérifie si le flag No-Execute est actif
    pub fn is_no_execute(&self) -> bool {
        self.0 & (1u64 << 63) != 0
    }
    
    /// Vérifie si le flag Copy-on-Write est actif
    pub fn is_cow(&self) -> bool {
        self.0 & 0x200 != 0
    }
    
    /// Convertit depuis une protection de page
    pub fn from_protection(protection: PageProtection) -> Self {
        let mut flags = Self::new().present();
        
        if protection.can_write() {
            flags = flags.writable();
        }
        
        if protection.is_user() {
            flags = flags.user();
        }
        
        if !protection.can_execute() {
            flags = flags.no_execute();
        }
        
        flags
    }
    
    /// Convertit vers une protection de page
    pub fn to_protection(&self) -> PageProtection {
        let mut protection = PageProtection::new();
        
        if self.is_user() {
            protection = protection.user();
        }
        
        if self.is_writable() {
            protection = protection.write();
        }
        
        if !self.is_no_execute() {
            protection = protection.execute();
        }
        
        protection.read() // Par défaut, les pages présentes sont lisibles
    }
}

/// Statistiques de la mémoire virtuelle
#[derive(Debug)]
pub struct VirtualMemoryStats {
    /// Nombre total de pages virtuelles
    pub total_pages: usize,
    /// Nombre de pages présentes en mémoire
    pub present_pages: usize,
    /// Nombre de pages swapées (si applicable)
    pub swapped_pages: usize,
    /// Nombre de pages Copy-on-Write
    pub cow_pages: usize,
    /// Nombre de fautes de page (page faults)
    pub page_faults: AtomicUsize,
    /// Nombre de fautes de page mineures (résolues sans I/O)
    pub minor_faults: AtomicUsize,
    /// Nombre de fautes de page majeures (résolues avec I/O)
    pub major_faults: AtomicUsize,
}

impl VirtualMemoryStats {
    /// Crée de nouvelles statistiques
    pub const fn new() -> Self {
        Self {
            total_pages: 0,
            present_pages: 0,
            swapped_pages: 0,
            cow_pages: 0,
            page_faults: AtomicUsize::new(0),
            minor_faults: AtomicUsize::new(0),
            major_faults: AtomicUsize::new(0),
        }
    }
    
    /// Incrémente le compteur de fautes de page
    pub fn inc_page_faults(&self) {
        self.page_faults.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Incrémente le compteur de fautes de page mineures
    pub fn inc_minor_faults(&self) {
        self.minor_faults.fetch_add(1, Ordering::Relaxed);
        self.inc_page_faults();
    }
    
    /// Incrémente le compteur de fautes de page majeures
    pub fn inc_major_faults(&self) {
        self.major_faults.fetch_add(1, Ordering::Relaxed);
        self.inc_page_faults();
    }
}

pub mod page_table;
pub mod mapper;
pub mod address_space;
pub mod cow;

use page_table::PageTable;

// Re-export AddressSpace and related types
pub use address_space::{
    AddressSpace, AddressSpaceStats, MemoryRegion, MemoryType, MemoryRegionType,
    VmSpace, VmFlags, VmPerms, VmArea
};

/// Initialise le sous-système de mémoire virtuelle
pub fn init() -> MemoryResult<()> {
    // Initialiser les structures de pagination du noyau
    page_table::init()?;
    
    // Initialiser le mapper
    mapper::init()?;
    
    // Initialiser le système Copy-on-Write
    cow::init()?;
    
    log::info!("Virtual memory subsystem initialized");
    Ok(())
}

/// Mappe une page virtuelle à une page physique
pub fn map_page(
    virtual_addr: VirtualAddress,
    physical_addr: PhysicalAddress,
    protection: PageProtection,
) -> MemoryResult<()> {
    let flags = PageTableFlags::from_protection(protection);
    mapper::map_page(virtual_addr, physical_addr, flags)
}

/// Démappe une page virtuelle
pub fn unmap_page(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    mapper::unmap_page(virtual_addr)
}

/// Change les protections d'une page
pub fn protect_page(
    virtual_addr: VirtualAddress,
    protection: PageProtection,
) -> MemoryResult<()> {
    let flags = PageTableFlags::from_protection(protection);
    mapper::protect_page(virtual_addr, flags)
}

/// Mappe une plage de pages
pub fn map_range(
    start_addr: VirtualAddress,
    physical_addr: PhysicalAddress,
    size: usize,
    protection: PageProtection,
) -> MemoryResult<()> {
    let flags = PageTableFlags::from_protection(protection);
    mapper::map_range(start_addr, physical_addr, size, flags)
}

/// Démappe une plage de pages
pub fn unmap_range(start_addr: VirtualAddress, size: usize) -> MemoryResult<()> {
    mapper::unmap_range(start_addr, size)
}

/// Change les protections d'une plage de pages
pub fn protect_range(
    start_addr: VirtualAddress,
    size: usize,
    protection: PageProtection,
) -> MemoryResult<()> {
    let flags = PageTableFlags::from_protection(protection);
    mapper::protect_range(start_addr, size, flags)
}

/// Obtient l'adresse physique correspondant à une adresse virtuelle
pub fn get_physical_address(virtual_addr: VirtualAddress) -> MemoryResult<Option<PhysicalAddress>> {
    mapper::get_physical_address(virtual_addr)
}

/// Vérifie si une page est présente
pub fn is_page_present(virtual_addr: VirtualAddress) -> MemoryResult<bool> {
    mapper::is_page_present(virtual_addr)
}

/// Vérifie si une page est Copy-on-Write
pub fn is_page_cow(virtual_addr: VirtualAddress) -> MemoryResult<bool> {
    mapper::is_page_cow(virtual_addr)
}

/// Marque une page comme Copy-on-Write
pub fn set_cow(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    mapper::set_cow(virtual_addr)
}

/// Gère une faute de page
pub fn handle_page_fault(virtual_addr: VirtualAddress, error_code: u64) -> MemoryResult<()> {
    // Incrémenter les statistiques
    let stats = get_stats();
    stats.inc_page_faults();
    
    // Déterminer le type de faute de page
    let is_present = (error_code & 0x1) != 0;
    let is_write = (error_code & 0x2) != 0;
    let _is_user = (error_code & 0x4) != 0;
    
    if !is_present {
        // Page non présente
        if is_write {
            // Écriture sur une page non présente (probablement CoW)
            cow::handle_cow_fault(virtual_addr)?;
            stats.inc_minor_faults();
        } else {
            // Lecture sur une page non présente
            // TODO: Gérer le chargement depuis le disque ou le swap
            return Err(MemoryError::InvalidAddress);
        }
    } else if is_write {
        // Écriture sur une page présente mais protégée en écriture (CoW)
        cow::handle_cow_fault(virtual_addr)?;
        stats.inc_minor_faults();
    } else {
        // Violation de protection (ex: exécution sur page NX)
        crate::memory::protection::handle_protection_violation(virtual_addr)?;
    }
    
    Ok(())
}

/// Crée un nouvel espace d'adressage
pub fn create_address_space() -> MemoryResult<AddressSpace> {
    address_space::create()
}

/// Détruit un espace d'adressage
pub fn destroy_address_space(address_space: AddressSpace) -> MemoryResult<()> {
    address_space::destroy(address_space)
}

/// Bascule vers un espace d'adressage
pub fn switch_address_space(address_space: &AddressSpace) -> MemoryResult<()> {
    address_space::switch(address_space)
}

/// Retourne l'espace d'adressage actuel
pub fn current_address_space() -> MemoryResult<AddressSpace> {
    address_space::current()
}

/// Retourne les statistiques actuelles de la mémoire virtuelle
pub fn get_stats() -> &'static VirtualMemoryStats {
    static STATS: VirtualMemoryStats = VirtualMemoryStats::new();
    &STATS
}

/// Invalide une entrée TLB pour une adresse spécifique
pub fn invalidate_tlb(virtual_addr: VirtualAddress) {
    arch::mmu::invalidate_tlb(virtual_addr.value());
}

/// Invalide toutes les entrées TLB
pub fn invalidate_tlb_all() {
    arch::mmu::invalidate_tlb_all();
}
