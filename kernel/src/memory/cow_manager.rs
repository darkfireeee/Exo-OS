//! Copy-on-Write Manager
//!
//! Gère le partage de pages mémoire avec Copy-on-Write entre processus.
//! Utilisé principalement par fork() pour cloner l'espace d'adressage efficacement.
//!
//! # Architecture
//!
//! - **Refcount tracking**: Compteur de références par page physique
//! - **CoW marking**: Pages marquées read-only dans page tables
//! - **Fault handling**: Page fault handler détecte write sur CoW page
//! - **Copy on demand**: Copie la page seulement quand nécessaire
//!
//! # Performance
//!
//! - O(1) refcount lookup (BTreeMap)
//! - Zero copy tant que pas de write
//! - Lazy allocation

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::memory::{PhysicalAddress, VirtualAddress, PAGE_SIZE};
use crate::memory::user_space::UserPageFlags as PageTableFlags;
use crate::memory::physical::{allocate_frame, deallocate_frame, Frame};
use crate::sync::Mutex;

/// Erreurs CoW
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowError {
    /// Mémoire insuffisante
    OutOfMemory,
    /// Page non CoW
    NotCowPage,
}

/// Entrée de reference counting
struct RefCountEntry {
    /// Compteur de références (atomique pour thread-safety)
    refcount: AtomicU32,
}

impl RefCountEntry {
    fn new(count: u32) -> Self {
        Self {
            refcount: AtomicU32::new(count),
        }
    }

    fn get(&self) -> u32 {
        self.refcount.load(Ordering::SeqCst)
    }

    fn increment(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn decrement(&self) -> u32 {
        self.refcount.fetch_sub(1, Ordering::SeqCst) - 1
    }
}

/// Gestionnaire de Copy-on-Write
pub struct CowManager {
    /// Référence counting par page physique
    refcounts: BTreeMap<PhysicalAddress, RefCountEntry>,
}

impl CowManager {
    /// Créer un nouveau gestionnaire CoW
    pub const fn new() -> Self {
        Self {
            refcounts: BTreeMap::new(),
        }
    }

    /// Marquer une page comme CoW
    ///
    /// # Arguments
    ///
    /// * `phys` - Adresse physique de la page
    ///
    /// # Returns
    ///
    /// Nouveau refcount après marquage
    pub fn mark_cow(&mut self, phys: PhysicalAddress) -> u32 {
        if let Some(entry) = self.refcounts.get(&phys) {
            // Déjà CoW: incrémenter
            entry.increment()
        } else {
            // Première fois: créer avec refcount=2 (partage initial)
            // Car on marque CoW quand on partage entre 2 processus
            self.refcounts.insert(phys, RefCountEntry::new(2));
            2
        }
    }

    /// Vérifier si une page est CoW
    pub fn is_cow(&self, phys: PhysicalAddress) -> bool {
        self.refcounts.contains_key(&phys)
    }

    /// Obtenir le refcount d'une page
    pub fn get_refcount(&self, phys: PhysicalAddress) -> Option<u32> {
        self.refcounts.get(&phys).map(|e| e.get())
    }

    /// Décrémenter le refcount d'une page CoW
    ///
    /// # Returns
    ///
    /// Nouveau refcount (retirer de tracking si 0)
    fn decrement(&mut self, phys: PhysicalAddress) -> u32 {
        if let Some(entry) = self.refcounts.get(&phys) {
            let new_count = entry.decrement();
            if new_count == 0 {
                self.refcounts.remove(&phys);
            }
            new_count
        } else {
            0
        }
    }

    /// Gérer un page fault CoW
    ///
    /// # Arguments
    ///
    /// * `virt` - Adresse virtuelle faultante
    /// * `phys` - Adresse physique actuelle
    ///
    /// # Returns
    ///
    /// Nouvelle adresse physique (copie privée) ou erreur
    pub fn handle_cow_fault(&mut self, virt: VirtualAddress, phys: PhysicalAddress) 
        -> Result<PhysicalAddress, CowError> 
    {
        // Vérifier que c'est bien une page CoW
        if !self.is_cow(phys) {
            return Err(CowError::NotCowPage);
        }

        // Si refcount == 1, juste retirer CoW (seul propriétaire)
        if let Some(count) = self.get_refcount(phys) {
            if count == 1 {
                self.refcounts.remove(&phys);
                return Ok(phys);
            }
        }

        // Sinon, copier la page
        self.copy_page(phys)
    }

    /// Copier une page physique
    ///
    /// # Arguments
    ///
    /// * `src_phys` - Adresse physique source
    ///
    /// # Returns
    ///
    /// Adresse physique de la nouvelle page (copie)
    fn copy_page(&mut self, src_phys: PhysicalAddress) -> Result<PhysicalAddress, CowError> {
        // Allouer nouvelle frame
        let new_frame = allocate_frame()
            .map_err(|_| CowError::OutOfMemory)?;

        let new_phys = new_frame.address();

        // Copier contenu (4096 bytes)
        unsafe {
            let src = src_phys.value() as *const u8;
            let dst = new_phys.value() as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
        }

        // Décrémenter refcount de la page source
        self.decrement(src_phys);

        Ok(new_phys)
    }

    /// Cloner l'espace d'adressage d'un processus (fork)
    ///
    /// # Arguments
    ///
    /// * `pages` - Liste de (virt, phys, flags) du parent
    ///
    /// # Returns
    ///
    /// Liste de (virt, phys, flags) pour le child, avec CoW
    pub fn clone_address_space(&mut self, pages: &[(VirtualAddress, PhysicalAddress, PageTableFlags)]) 
        -> Result<alloc::vec::Vec<(VirtualAddress, PhysicalAddress, PageTableFlags)>, CowError> 
    {
        let mut new_pages = alloc::vec::Vec::with_capacity(pages.len());

        for &(virt, phys, flags) in pages {
            // Si page writable, la marquer CoW
            if flags.contains_writable() {
                self.mark_cow(phys);
                
                // Créer mapping read-only pour parent ET child
                let cow_flags = flags.remove_writable();
                new_pages.push((virt, phys, cow_flags));
            } else {
                // Page déjà read-only: partager sans CoW
                new_pages.push((virt, phys, flags));
            }
        }

        Ok(new_pages)
    }

    /// Libérer une page CoW
    ///
    /// # Arguments
    ///
    /// * `phys` - Adresse physique de la page
    ///
    /// # Behavior
    ///
    /// Décrémente le refcount. Si refcount atteint 0, libère la frame physique.
    pub fn free_cow_page(&mut self, phys: PhysicalAddress) {
        let new_refcount = self.decrement(phys);
        
        if new_refcount == 0 {
            let frame = Frame::containing_address(phys);
            let _ = deallocate_frame(frame);
        }
    }

    /// Nombre de pages CoW trackées
    pub fn tracked_pages(&self) -> usize {
        self.refcounts.len()
    }
}

/// Gestionnaire global CoW (singleton)
static COW_MANAGER: Mutex<CowManager> = Mutex::new(CowManager::new());

/// Marquer une page comme CoW
pub fn mark_cow(phys: PhysicalAddress) -> u32 {
    COW_MANAGER.lock().mark_cow(phys)
}

/// Vérifier si une page est CoW
pub fn is_cow(phys: PhysicalAddress) -> bool {
    COW_MANAGER.lock().is_cow(phys)
}

/// Gérer un page fault CoW
pub fn handle_cow_fault(virt: VirtualAddress, phys: PhysicalAddress) 
    -> Result<PhysicalAddress, CowError> 
{
    COW_MANAGER.lock().handle_cow_fault(virt, phys)
}

/// Libérer une page CoW
pub fn free_cow_page(phys: PhysicalAddress) {
    COW_MANAGER.lock().free_cow_page(phys)
}

/// Cloner l'espace d'adressage avec CoW
pub fn clone_address_space(pages: &[(VirtualAddress, PhysicalAddress, PageTableFlags)]) 
    -> Result<alloc::vec::Vec<(VirtualAddress, PhysicalAddress, PageTableFlags)>, CowError> 
{
    COW_MANAGER.lock().clone_address_space(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cow_refcount() {
        let mut manager = CowManager::new();
        let phys = PhysicalAddress::new(0x1000);

        // Premier mark_cow: refcount = 2 (partage initial)
        let count1 = manager.mark_cow(phys);
        assert_eq!(count1, 2);

        // Deuxième mark_cow: refcount = 3 (3ème partage)
        let count2 = manager.mark_cow(phys);
        assert_eq!(count2, 3);

        assert!(manager.is_cow(phys));
    }

    #[test]
    fn test_cow_decrement() {
        let mut manager = CowManager::new();
        let phys = PhysicalAddress::new(0x2000);

        // Mark CoW 2 fois: refcount = 2, puis 3
        manager.mark_cow(phys);
        manager.mark_cow(phys);

        // Décrémente: 3 → 2
        let count1 = manager.decrement(phys);
        assert_eq!(count1, 2);

        // Décrémente: 2 → 1
        let count2 = manager.decrement(phys);
        assert_eq!(count2, 1);

        // Décrémente: 1 → 0 (retiré)
        let count3 = manager.decrement(phys);
        assert_eq!(count3, 0);

        // Après refcount 0, doit être retiré
        assert!(!manager.is_cow(phys));
    }

    #[test]
    fn test_cow_not_cow_page() {
        let mut manager = CowManager::new();
        let phys = PhysicalAddress::new(0x3000);
        let virt = VirtualAddress::new(0x400000);

        // Essayer de handle fault sur page non-CoW
        let result = manager.handle_cow_fault(virt, phys);
        assert_eq!(result, Err(CowError::NotCowPage));
    }

    #[test]
    fn test_cow_tracked_pages() {
        let mut manager = CowManager::new();
        let phys1 = PhysicalAddress::new(0x1000);
        let phys2 = PhysicalAddress::new(0x2000);

        assert_eq!(manager.tracked_pages(), 0);

        // Mark phys1: refcount=2, 1 page trackée
        manager.mark_cow(phys1);
        assert_eq!(manager.tracked_pages(), 1);

        // Mark phys2: refcount=2, 2 pages trackées
        manager.mark_cow(phys2);
        assert_eq!(manager.tracked_pages(), 2);

        // Decrement phys1: 2→1, encore trackée
        manager.decrement(phys1);
        assert_eq!(manager.tracked_pages(), 2);

        // Decrement phys1: 1→0, retirée
        manager.decrement(phys1);
        assert_eq!(manager.tracked_pages(), 1);
    }
}
