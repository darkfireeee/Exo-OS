// kernel/src/memory/virtual/page_table/builder.rs
//
// Builder de table de pages — construction incrémentale d'une PML4.
// Interface fluente pour les mappings kernel et user au boot.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{
    AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::memory::virt::page_table::walker::{FrameAllocatorForWalk, PageTableWalker};
use crate::memory::virt::page_table::x86_64::{phys_to_table_mut, read_cr3};

// ─────────────────────────────────────────────────────────────────────────────
// BUILDER DE TABLE DE PAGES
// ─────────────────────────────────────────────────────────────────────────────

/// Builder permettant de construire une table de pages complète.
pub struct PageTableBuilder<'a, A: FrameAllocatorForWalk> {
    pml4_phys: PhysAddr,
    alloc: &'a A,
    walker: PageTableWalker,
}

impl<'a, A: FrameAllocatorForWalk> PageTableBuilder<'a, A> {
    /// Crée un builder avec une PML4 fraîchement allouée.
    pub fn new(alloc: &'a A) -> Result<Self, AllocError> {
        let pml4_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
        let pml4_phys = pml4_frame.start_address();
        Ok(PageTableBuilder {
            pml4_phys,
            alloc,
            walker: PageTableWalker::new(pml4_phys),
        })
    }

    /// Crée un builder sur une PML4 existante.
    pub fn from_existing(pml4_phys: PhysAddr, alloc: &'a A) -> Self {
        PageTableBuilder {
            pml4_phys,
            alloc,
            walker: PageTableWalker::new(pml4_phys),
        }
    }

    /// Mappe une seule page.
    pub fn map_page(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<&mut Self, AllocError> {
        self.walker.map(virt, frame, flags, self.alloc)?;
        Ok(self)
    }

    /// Mappe une plage physique contiguë vers une plage virtuelle contiguë.
    pub fn map_range(
        &mut self,
        virt_start: VirtAddr,
        phys_start: PhysAddr,
        size_bytes: usize,
        flags: PageFlags,
    ) -> Result<&mut Self, AllocError> {
        let n_pages = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..n_pages {
            let v = VirtAddr::new(virt_start.as_u64() + (i * PAGE_SIZE) as u64);
            let p = PhysAddr::new(phys_start.as_u64() + (i * PAGE_SIZE) as u64);
            self.walker
                .map(v, Frame::containing(p), flags, self.alloc)?;
        }
        Ok(self)
    }

    /// Mappe tout le physmap kernel (PHYS_MAP_BASE → physique 0).
    /// `phys_size` est la taille totale de RAM détectée.
    pub fn map_physmap(&mut self, phys_size: u64) -> Result<&mut Self, AllocError> {
        let phys_map_base = crate::memory::core::layout::PHYS_MAP_BASE;
        let n_pages = (phys_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
        let flags = PageFlags::KERNEL_DATA;
        for i in 0..n_pages {
            let v = VirtAddr::new(phys_map_base.as_u64() + (i * PAGE_SIZE) as u64);
            let p = PhysAddr::new((i * PAGE_SIZE) as u64);
            self.walker
                .map(v, Frame::containing(p), flags, self.alloc)?;
        }
        Ok(self)
    }

    /// Clone les entrées kernel (PML4[256..512]) depuis la PML4 active.
    /// Utilisé pour créer l'espace utilisateur qui partage le noyau.
    ///
    /// SAFETY: Doit être appelé quand la PML4 active est complète et valide.
    pub unsafe fn copy_kernel_entries(&mut self) -> &mut Self {
        let current_pml4_phys = read_cr3();
        let current_pml4 = phys_to_table_mut(current_pml4_phys);
        let new_pml4 = phys_to_table_mut(self.pml4_phys);
        // Entrées kernel : indices 256..512 (moitié haute de l'espace 48 bits)
        for i in 256..512 {
            new_pml4[i] = current_pml4[i];
        }
        self
    }

    /// Efface la moitié utilisateur de la PML4 (indices 0..256).
    ///
    /// SAFETY: pml4_phys pointe sur une PML4 valide.
    pub unsafe fn clear_user_entries(&mut self) -> &mut Self {
        let pml4 = phys_to_table_mut(self.pml4_phys);
        for i in 0..256 {
            pml4[i].clear();
        }
        self
    }

    /// Retourne l'adresse physique de la PML4 construite.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Consomme le builder et retourne la PML4.
    pub fn build(self) -> PhysAddr {
        self.pml4_phys
    }
}
