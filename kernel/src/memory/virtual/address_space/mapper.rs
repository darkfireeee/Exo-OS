// kernel/src/memory/virtual/address_space/mapper.rs
//
// Mapper d'espace d'adressage — interface haut niveau pour map/unmap/remap.
// Gère les invalidations TLB automatiquement après chaque modification.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::tlb::{flush_range, flush_single};
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker, WalkResult};

// ─────────────────────────────────────────────────────────────────────────────
// MAPPER
// ─────────────────────────────────────────────────────────────────────────────

/// Mapper des pages virtuelles dans un espace d'adressage spécifique.
/// Wrapping autour de PageTableWalker avec gestion automatique du TLB.
pub struct Mapper<'a, A: FrameAllocatorForWalk> {
    walker: PageTableWalker,
    alloc: &'a A,
    /// Nombre de pages mappées depuis la création (pour stats).
    mapped_count: u64,
    unmapped_count: u64,
}

impl<'a, A: FrameAllocatorForWalk> Mapper<'a, A> {
    /// Crée un mapper pour la PML4 à `pml4_phys`.
    pub fn new(pml4_phys: PhysAddr, alloc: &'a A) -> Self {
        Mapper {
            walker: PageTableWalker::new(pml4_phys),
            alloc,
            mapped_count: 0,
            unmapped_count: 0,
        }
    }

    /// Mappe une page virtuelle vers un frame physique.
    ///
    /// Invalide automatiquement le TLB local pour `virt`.
    pub fn map(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        self.walker.map(virt, frame, flags, self.alloc)?;
        // SAFETY: virt est une adresse canonique (validée par PageTableWalker).
        unsafe {
            flush_single(virt);
        }
        self.mapped_count += 1;
        Ok(())
    }

    /// Mappe une plage physique contiguë vers une plage virtuelle contiguë.
    pub fn map_range(
        &mut self,
        virt_start: VirtAddr,
        phys_start: PhysAddr,
        size_bytes: usize,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        let n = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..n {
            let v = VirtAddr::new(virt_start.as_u64() + (i * PAGE_SIZE) as u64);
            let p = PhysAddr::new(phys_start.as_u64() + (i * PAGE_SIZE) as u64);
            self.walker
                .map(v, Frame::containing(p), flags, self.alloc)?;
            // SAFETY: adresses canoniques.
            unsafe {
                flush_single(v);
            }
        }
        self.mapped_count += n as u64;
        Ok(())
    }

    /// Démappe une page virtuelle.
    ///
    /// Retourne le frame précédement mappé, ou `None`.
    /// Invalide le TLB local.
    pub fn unmap(&mut self, virt: VirtAddr) -> Option<Frame> {
        let result = self.walker.unmap(virt);
        if result.is_some() {
            // SAFETY: adresse canonique.
            unsafe {
                flush_single(virt);
            }
            self.unmapped_count += 1;
        }
        result
    }

    /// Démappe une plage de pages.
    /// Retourne le nombre de pages effectivement démappées.
    pub fn unmap_range(&mut self, start: VirtAddr, size_bytes: usize) -> usize {
        let n = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut count = 0usize;
        for i in 0..n {
            let v = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            if self.walker.unmap(v).is_some() {
                count += 1;
            }
        }
        if count > 0 {
            // SAFETY: plage d'adresses canoniques.
            unsafe {
                flush_range(start, VirtAddr::new(start.as_u64() + size_bytes as u64));
            }
        }
        self.unmapped_count += count as u64;
        count
    }

    /// Modifie les flags d'une page existante.
    pub fn remap_flags(&mut self, virt: VirtAddr, new_flags: PageFlags) -> Result<(), AllocError> {
        self.walker.remap_flags(virt, new_flags)?;
        // SAFETY: adresse canonique.
        unsafe {
            flush_single(virt);
        }
        Ok(())
    }

    /// Vérifie si une adresse est mappée.
    pub fn is_mapped(&self, virt: VirtAddr) -> bool {
        matches!(
            self.walker.walk_read(virt),
            WalkResult::Leaf { .. } | WalkResult::HugePage { .. }
        )
    }

    /// Traduit une adresse virtuelle en adresse physique.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        match self.walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let page_offset = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + page_offset))
            }
            WalkResult::HugePage { entry, level } => {
                let page_size = level.page_size() as u64;
                let offset = virt.as_u64() & (page_size - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + offset))
            }
            _ => None,
        }
    }

    /// Retourne les statistiques du mapper.
    pub fn stats(&self) -> (u64, u64) {
        (self.mapped_count, self.unmapped_count)
    }
}
