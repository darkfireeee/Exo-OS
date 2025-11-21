//! # Espaces d'Adressage par Processus
//! 
//! Ce module gère les espaces d'adressage virtuels pour chaque processus,
//! incluant la création, la destruction et le basculement entre les espaces.

extern crate alloc;
use alloc::vec::Vec;

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryResult, MemoryError, PageProtection};
use crate::arch;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Représente une région dans un espace d'adressage
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Adresse de début de la région
    pub start: VirtualAddress,
    /// Taille de la région en octets
    pub size: usize,
    /// Permissions de la région
    pub protection: PageProtection,
    /// Type de région
    pub region_type: MemoryRegionType,
    /// Informations supplémentaires (ex: offset de fichier)
    pub info: MemoryRegionInfo,
}

/// Types de régions mémoire
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryRegionType {
    /// Code du programme
    Code,
    /// Données du programme
    Data,
    /// Tas (heap)
    Heap,
    /// Pile (stack)
    Stack,
    /// Mémoire mmap
    Mmap,
    /// Bibliothèques partagées
    SharedLibrary,
    /// Région anonyme
    Anonymous,
    /// Région du noyau
    Kernel,
}

/// Alias pour MemoryRegionType (compatibilité)
pub type MemoryType = MemoryRegionType;

/// Alias pour AddressSpace (compatibilité VM)
pub type VmSpace = AddressSpace;

/// Alias pour PageProtection (compatibilité VM)
pub type VmPerms = PageProtection;

/// Alias pour MemoryRegion (compatibilité VM)
pub type VmArea = MemoryRegion;

/// Flags pour les opérations VM (compatibilité)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VmFlags(u32);

impl VmFlags {
    pub const SHARED: Self = Self(1 << 0);
    pub const PRIVATE: Self = Self(1 << 1);
    pub const FIXED: Self = Self(1 << 2);
    pub const ANONYMOUS: Self = Self(1 << 3);
}

/// Informations supplémentaires sur une région mémoire
#[derive(Debug, Clone)]
pub enum MemoryRegionInfo {
    /// Aucune information supplémentaire
    None,
    /// Informations sur une pile
    Stack {
        /// Pointeur de base de la pile
        base: VirtualAddress,
        /// Taille maximale de la pile
        max_size: usize,
    },
    /// Informations sur une région mmap
    Mmap {
        /// Descripteur de fichier (si applicable)
        file_descriptor: Option<usize>,
        /// Offset dans le fichier
        offset: usize,
    },
    /// Informations sur une bibliothèque partagée
    SharedLibrary {
        /// Nom de la bibliothèque
        name: &'static str,
        /// Adresse de base de la bibliothèque
        base: VirtualAddress,
    },
}

impl MemoryRegion {
    /// Crée une nouvelle région mémoire
    pub fn new(
        start: VirtualAddress,
        size: usize,
        protection: PageProtection,
        region_type: MemoryRegionType,
        info: MemoryRegionInfo,
    ) -> Self {
        Self {
            start,
            size,
            protection,
            region_type,
            info,
        }
    }
    
    /// Retourne l'adresse de fin de la région (exclusive)
    pub fn end(&self) -> VirtualAddress {
        VirtualAddress::new(self.start.value() + self.size)
    }
    
    /// Vérifie si une adresse est dans cette région
    pub fn contains(&self, address: VirtualAddress) -> bool {
        address.value() >= self.start.value() && address.value() < self.end().value()
    }
    
    /// Vérifie si cette région chevauche une autre
    pub fn overlaps(&self, other: &MemoryRegion) -> bool {
        self.start.value() < other.end().value() && other.start.value() < self.end().value()
    }
}

/// Représente un espace d'adressage virtuel
#[derive(Debug)]
pub struct AddressSpace {
    /// Racine de la hiérarchie des tables de pages (adresse physique du PML4)
    root_address: PhysicalAddress,
    /// Liste des régions mémoire dans cet espace d'adressage
    regions: Vec<MemoryRegion>,
    /// Identifiant unique de cet espace d'adressage
    id: usize,
    /// Statistiques de cet espace d'adressage
    stats: AddressSpaceStats,
}

/// Statistiques d'un espace d'adressage
#[derive(Debug)]
pub struct AddressSpaceStats {
    /// Nombre de pages utilisées
    pub used_pages: AtomicUsize,
    /// Taille totale de l'espace d'adressage
    pub total_size: usize,
    /// Taille utilisée de l'espace d'adressage
    pub used_size: usize,
}

impl AddressSpaceStats {
    /// Crée de nouvelles statistiques
    pub const fn new() -> Self {
        Self {
            used_pages: AtomicUsize::new(0),
            total_size: 0,
            used_size: 0,
        }
    }
    
    /// Incrémente le compteur de pages utilisées
    pub fn inc_used_pages(&self) {
        self.used_pages.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Décrémente le compteur de pages utilisées
    pub fn dec_used_pages(&self) {
        self.used_pages.fetch_sub(1, Ordering::Relaxed);
    }
}

impl AddressSpace {
    /// Crée un nouvel espace d'adressage
    pub fn new() -> MemoryResult<Self> {
        // Créer une nouvelle table de pages racine
        let root_table = super::page_table::PageTable::new(super::page_table::PAGE_TABLE_LEVELS - 1)?;
        
        // Créer l'espace d'adressage
        let mut address_space = Self {
            root_address: root_table.physical_address(),
            regions: Vec::new(),
            id: generate_address_space_id(),
            stats: AddressSpaceStats::new(),
        };
        
        // Mapper le noyau dans le nouvel espace d'adressage
        address_space.map_kernel()?;
        
        // La table racine sera libérée quand l'espace d'adressage est détruit
        core::mem::forget(root_table);
        
        Ok(address_space)
    }
    
    /// Mappe le noyau dans cet espace d'adressage
    fn map_kernel(&mut self) -> MemoryResult<()> {
        let kernel_start = crate::arch::KERNEL_START_ADDRESS;
        let kernel_end = crate::arch::KERNEL_END_ADDRESS;
        let kernel_size = (kernel_end - kernel_start) as usize;
        
        let mut mapper = super::mapper::MemoryMapper::for_address_space(self.root_address)?;
        
        // Mapper le code et les données du noyau
        for offset in (0..kernel_size).step_by(arch::PAGE_SIZE) {
            let virtual_addr = VirtualAddress::new((kernel_start as usize) + offset);
            let physical_addr = PhysicalAddress::new(virtual_addr.value() - (crate::arch::KERNEL_VIRTUAL_OFFSET as usize));
            
            // Si c'est dans la région du code du noyau, autoriser l'exécution
            let flags = if virtual_addr.value() >= (crate::arch::KERNEL_CODE_START as usize)
                && virtual_addr.value() < (crate::arch::KERNEL_CODE_END as usize) {
                super::page_table::PageTableFlags::new()
                    .present()
                    .writable()
                    .global()
                    .execute()
            } else {
                super::page_table::PageTableFlags::new()
                    .present()
                    .writable()
                    .global()
                    .no_execute()
            };
            
            mapper.map_page(virtual_addr, physical_addr, flags)?;
        }
        
        // Ajouter la région du noyau à la liste des régions
        let kernel_region = MemoryRegion::new(
            VirtualAddress::new(kernel_start as usize),
            kernel_size,
            PageProtection::new().read().write().execute(),
            MemoryRegionType::Kernel,
            MemoryRegionInfo::None,
        );
        
        self.regions.push(kernel_region);
        
        Ok(())
    }
    
    /// Retourne l'adresse de la table de pages racine
    pub fn root_address(&self) -> PhysicalAddress {
        self.root_address
    }
    
    /// Retourne l'identifiant de cet espace d'adressage
    pub fn id(&self) -> usize {
        self.id
    }
    
    /// Retourne la liste des régions mémoire
    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }
    
    /// Ajoute une région mémoire à cet espace d'adressage
    pub fn add_region(&mut self, region: MemoryRegion) -> MemoryResult<()> {
        // Vérifier si la région chevauche des régions existantes
        for existing_region in &self.regions {
            if region.overlaps(existing_region) {
                return Err(MemoryError::InvalidAddress);
            }
        }
        
        let region_size = region.size;
        self.regions.push(region);
        self.stats.used_size += region_size;
        
        Ok(())
    }
    
    /// Supprime une région mémoire de cet espace d'adressage
    pub fn remove_region(&mut self, start: VirtualAddress, size: usize) -> MemoryResult<MemoryRegion> {
        // Trouver la région à supprimer
        let index = self.regions.iter().position(|r| {
            r.start == start && r.size == size
        }).ok_or(MemoryError::InvalidAddress)?;
        
        let region = self.regions.remove(index);
        self.stats.used_size -= region.size;
        
        Ok(region)
    }
    
    /// Trouve une région libre pour une allocation
    pub fn find_free_region(&self, size: usize, hint: Option<VirtualAddress>) -> MemoryResult<VirtualAddress> {
        // Si une indication est fournie, essayer de l'utiliser
        if let Some(hint_addr) = hint {
            let end_addr = VirtualAddress::new(hint_addr.value() + size);
            
            // Vérifier si la région est libre
            let mut free = true;
            for region in &self.regions {
                if region.start.value() < end_addr.value() && region.end().value() > hint_addr.value() {
                    free = false;
                    break;
                }
            }
            
            if free {
                return Ok(hint_addr);
            }
        }
        
        // Chercher une région libre dans l'espace utilisateur
        // Pour simplifier, on utilise une stratégie simple
        let user_start = VirtualAddress::new(0x400000); // 4MB
        let user_end = VirtualAddress::new(crate::arch::KERNEL_BASE as usize);
        
        let mut current = user_start;
        while current.value() + size <= user_end.value() {
            let end = VirtualAddress::new(current.value() + size);
            
            // Vérifier si la région est libre
            let mut free = true;
            for region in &self.regions {
                if region.start.value() < end.value() && region.end().value() > current.value() {
                    free = false;
                    current = region.end();
                    break;
                }
            }
            
            if free {
                return Ok(current);
            }
        }
        
        Err(MemoryError::OutOfMemory)
    }

    /// Vérifie si une plage d'adresses est accessible avec les permissions données
    pub fn is_range_accessible(&self, start: crate::memory::address::UserVirtAddr, size: usize, write: bool, execute: bool) -> bool {
        let start_addr = VirtualAddress::new(start.as_usize());
        let end_addr = VirtualAddress::new(start.as_usize() + size);
        
        // Vérifier chaque région
        for region in &self.regions {
            // Si la région contient le début de la plage
            if region.contains(start_addr) {
                // Vérifier les permissions
                if write && !region.protection.can_write() {
                    return false;
                }
                if execute && !region.protection.can_execute() {
                    return false;
                }
                
                // Si la région couvre toute la plage, c'est bon
                if region.end() >= end_addr {
                    return true;
                }
                
                // Sinon, il faut vérifier la suite de la plage (cas complexe, simplifié ici)
                // Pour l'instant, on suppose que la plage doit être contenue dans une seule région
                return false;
            }
        }
        
        false
    }
    
    /// Retourne les statistiques de cet espace d'adressage
    pub fn stats(&self) -> &AddressSpaceStats {
        &self.stats
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // Libérer la table de pages racine
        let frame = crate::memory::physical::Frame::containing_address(self.root_address);
        let _ = crate::memory::physical::deallocate_frame(frame);
    }
}

/// Compteur pour générer des identifiants uniques d'espaces d'adressage
static ADDRESS_SPACE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Génère un identifiant unique pour un espace d'adressage
fn generate_address_space_id() -> usize {
    ADDRESS_SPACE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Crée un nouvel espace d'adressage
pub fn create() -> MemoryResult<AddressSpace> {
    AddressSpace::new()
}

/// Détruit un espace d'adressage
pub fn destroy(address_space: AddressSpace) -> MemoryResult<()> {
    // L'espace d'adressage sera automatiquement détruit quand il sort du scope
    // grâce à l'implémentation de Drop
    drop(address_space);
    Ok(())
}

/// Bascule vers un espace d'adressage
pub fn switch(address_space: &AddressSpace) -> MemoryResult<()> {
    arch::mmu::set_page_table_root(address_space.root_address());
    Ok(())
}

/// Retourne l'espace d'adressage actuel
pub fn current() -> MemoryResult<AddressSpace> {
    let root_address = arch::mmu::get_page_table_root();
    
    // Pour simplifier, on crée un nouvel espace d'adressage à partir de la racine actuelle
    // En pratique, on devrait avoir un moyen de retrouver l'objet AddressSpace existant
    let address_space = AddressSpace {
        root_address,
        regions: Vec::new(), // TODO: Reconstruire la liste des régions
        id: 0, // TODO: Récupérer l'ID réel
        stats: AddressSpaceStats::new(), // TODO: Reconstruire les statistiques
    };
    
    // TODO: Reconstruire la liste des régions en parcourant les tables de pages
    
    Ok(address_space)
}
