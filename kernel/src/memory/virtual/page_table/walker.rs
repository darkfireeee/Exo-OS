// kernel/src/memory/virtual/page_table/walker.rs
//
// Walker de table de pages — parcours récursif PML4→PDPT→PD→PT.
// Gestion du walk avec allocation à la demande ou read-only.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::virt::page_table::x86_64::{
    phys_to_table_mut, phys_to_table_ref, PageTable, PageTableEntry, PageTableLevel,
};

// ─────────────────────────────────────────────────────────────────────────────
// RÉSULTAT D'UN WALK
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat du parcours d'une table de pages jusqu'à une feuille.
#[derive(Debug)]
pub enum WalkResult {
    /// Feuille trouvée (PT entré + index).
    Leaf {
        entry: PageTableEntry,
        level: PageTableLevel,
    },
    /// Page non présente à ce niveau.
    NotMapped,
    /// Huge page (2 MiB ou 1 GiB) détectée avant d'atteindre le niveau 1.
    HugePage {
        entry: PageTableEntry,
        level: PageTableLevel,
    },
    /// Erreur lors de l'allocation d'un niveau intermédiaire.
    AllocError(AllocError),
}

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT D'ALLOCATION POUR LE WALKER
// ─────────────────────────────────────────────────────────────────────────────

/// Permet au walker d'allouer des tables intermédiaires sans dépendre
/// directement du buddy allocator (inversion de dépendance).
pub trait FrameAllocatorForWalk {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError>;
    fn free_frame(&self, frame: Frame);
}

// ─────────────────────────────────────────────────────────────────────────────
// WALKER
// ─────────────────────────────────────────────────────────────────────────────

/// Walker de table de pages x86_64.
/// Parcourt les 4 niveaux pour trouver ou créer une mapping.
pub struct PageTableWalker {
    pml4_phys: PhysAddr,
}

impl PageTableWalker {
    /// Crée un walker pour la PML4 à `pml4_phys`.
    pub fn new(pml4_phys: PhysAddr) -> Self {
        PageTableWalker { pml4_phys }
    }

    /// Walk en lecture seule jusqu'à la feuille pour `virt`.
    /// Ne crée aucun niveau intermédiaire.
    pub fn walk_read(&self, virt: VirtAddr) -> WalkResult {
        // SAFETY: pml4_phys est une PML4 valide initialisée avant tout walk.
        let pml4 = unsafe { phys_to_table_ref(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return WalkResult::NotMapped;
        }

        // SAFETY: l4_entry.phys_addr() pointe sur un PDPT valide.
        let pdpt = unsafe { phys_to_table_ref(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return WalkResult::NotMapped;
        }
        if l3_entry.is_huge() {
            return WalkResult::HugePage {
                entry: l3_entry,
                level: PageTableLevel::L3,
            };
        }

        // SAFETY: l3_entry.phys_addr() pointe sur un PD valide.
        let pd = unsafe { phys_to_table_ref(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return WalkResult::NotMapped;
        }
        if l2_entry.is_huge() {
            return WalkResult::HugePage {
                entry: l2_entry,
                level: PageTableLevel::L2,
            };
        }

        // SAFETY: l2_entry.phys_addr() pointe sur un PT valide.
        let pt = unsafe { phys_to_table_ref(l2_entry.phys_addr()) };
        let l1_entry = pt[virt.p1_index()];
        if !l1_entry.is_present() {
            return WalkResult::NotMapped;
        }
        WalkResult::Leaf {
            entry: l1_entry,
            level: PageTableLevel::L1,
        }
    }

    /// Mappe `virt` → `frame` avec les flags donnés.
    ///
    /// Crée les niveaux intermédiaires manquants en utilisant `alloc`.
    /// Retourne `AllocError` si une page intermédiaire ne peut pas être allouée.
    pub fn map<A: FrameAllocatorForWalk>(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        // SAFETY: pml4_phys est une PML4 valide (initialisée avant).
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, virt.p4_index(), alloc, flags)?;
        // SAFETY: l3_phys pointe sur un PDPT valide ou fraîchement alloué.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, virt.p3_index(), alloc, flags)?;
        // SAFETY: l2_phys pointe sur un PD valide ou fraîchement alloué.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let l1_phys = self.ensure_table(pd, virt.p2_index(), alloc, flags)?;
        // SAFETY: l1_phys pointe sur un PT valide ou fraîchement alloué.
        let pt = unsafe { phys_to_table_mut(l1_phys) };

        let idx = virt.p1_index();
        let entry = &mut pt[idx];
        let new_entry = PageTableEntry::from_page_flags(frame, flags);
        *entry = new_entry;
        Ok(())
    }

    /// Démappate `virt`.
    ///
    /// Retourne le frame précédemment mappé, ou `None` si non mappé.
    /// Retourne également `None` si une huge page est rencontrée en chemin
    /// (utiliser un démappate spécifique huge page dans ce cas).
    pub fn unmap(&mut self, virt: VirtAddr) -> Option<Frame> {
        // SAFETY: pml4_phys est une PML4 valide.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return None;
        }

        // SAFETY: l4_entry.phys_addr() est un PDPT valide.
        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return None;
        }
        // Une huge page 1 GiB : on ne peut pas décomposer en 4 KiB ici
        if l3_entry.is_huge() {
            return None;
        }

        // SAFETY: l3_entry.phys_addr() est un PD valide.
        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return None;
        }
        // Une huge page 2 MiB : idem
        if l2_entry.is_huge() {
            return None;
        }

        // SAFETY: l2_entry.phys_addr() est un PT valide.
        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        let l1_idx = virt.p1_index();
        let old = pt[l1_idx];
        if !old.is_present() {
            return None;
        }
        pt[l1_idx].clear();
        old.frame()
    }

    /// Modifie les flags d'un mapping existant.
    pub fn remap_flags(&mut self, virt: VirtAddr, new_flags: PageFlags) -> Result<(), AllocError> {
        // SAFETY: pml4_phys est une PML4 valide.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // Huge page 1 GiB : cette fonction ne gère que les mappings 4 KiB
        if l3_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }

        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // Huge page 2 MiB : idem
        if l2_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }

        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        let entry = &mut pt[virt.p1_index()];
        if !entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        let frame = entry.frame().ok_or(AllocError::InvalidParams)?;
        *entry = PageTableEntry::from_page_flags(frame, new_flags);
        Ok(())
    }

    // ─── helper : s'assure qu'un niveau intermédiaire existe ─────────────────

    fn ensure_table<A: FrameAllocatorForWalk>(
        &self,
        parent: &mut PageTable,
        idx: usize,
        alloc: &A,
        flags: PageFlags,
    ) -> Result<PhysAddr, AllocError> {
        let entry = &mut parent[idx];
        if entry.is_present() {
            // Si l'entrée pointe vers une huge page (2 MiB / 1 GiB),
            // on ne peut pas la traiter comme une table intermédiaire.
            if entry.is_huge() {
                return Err(AllocError::InvalidParams);
            }
            return Ok(entry.phys_addr());
        }
        // Allouer un nouveau frame pour la table intermédiaire
        let new_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
        let new_phys = new_frame.start_address();
        // Initialiser à zéro (ZEROED flag garantit les 4 KiB à 0)
        // Écrire l'entrée avec droits minimaux (PRESENT | WRITABLE | USER si user)
        let parent_flags = PageTableEntry::FLAG_PRESENT
            | PageTableEntry::FLAG_WRITABLE
            | if flags.contains(PageFlags::USER) {
                PageTableEntry::FLAG_USER
            } else {
                0
            };
        *entry = PageTableEntry::from_raw(new_phys.as_u64() | parent_flags);
        Ok(new_phys)
    }
}
