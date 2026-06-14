// kernel/src/memory/virtual/page_table/walker.rs
//
// Walker de table de pages — parcours récursif PML4→PDPT→PD→PT.
// Gestion du walk avec allocation à la demande ou read-only.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::virt::page_table::x86_64::{
    phys_to_table_mut, phys_to_table_ref, PageTable, PageTableEntry, PageTableLevel,
};
use core::sync::atomic::{AtomicU64, Ordering};

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
        // DIAG-STKDBLMAP (temporaire) : un frame connu comme pile (≥0x7fff...) est-il
        // mappé ici sur une adresse NON-pile ? = double-mapping → l'autre AS écrit la
        // pile d'init (corruption → saut NULL). Couvre TOUS les chemins de map.
        if virt.as_u64() < 0x7fff_0000_0000
            && crate::memory::physical::allocator::buddy::stk_watch_is_watched(
                frame.start_address().as_u64(),
            )
        {
            let out = crate::arch::x86_64::terminal::debug_write;
            out(b"<STKDBLMAP va=");
            let hexd = b"0123456789abcdef";
            let mut b = [0u8; 12];
            let v = virt.as_u64();
            let mut i = 0;
            while i < 12 {
                b[i] = hexd[((v >> ((11 - i) * 4)) & 0xf) as usize];
                i += 1;
            }
            out(&b);
            out(b" f=");
            let f = frame.start_address().as_u64();
            let mut b2 = [0u8; 9];
            let mut j = 0;
            while j < 9 {
                b2[j] = hexd[((f >> ((8 - j) * 4)) & 0xf) as usize];
                j += 1;
            }
            out(&b2);
            out(b">");
        }
        *entry = new_entry;
        Ok(())
    }

    /// Mappe une huge page 2 MiB au niveau PD.
    ///
    /// Utilisé pour la physmap kernel: les tables intermédiaires sont créées
    /// normalement, mais la feuille est une entrée PD avec le bit PS.
    pub fn map_huge_2m<A: FrameAllocatorForWalk>(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        const HUGE_2M: u64 = 2 * 1024 * 1024;
        if (virt.as_u64() & (HUGE_2M - 1)) != 0 || !phys.is_aligned(HUGE_2M) {
            return Err(AllocError::InvalidParams);
        }

        let table_flags = flags.clear(PageFlags::HUGE_PAGE);
        let huge_flags = flags.set(PageFlags::HUGE_PAGE);
        // SAFETY: pml4_phys est une PML4 valide.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, virt.p4_index(), alloc, table_flags)?;
        // SAFETY: l3_phys pointe sur un PDPT valide ou fraîchement alloué.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, virt.p3_index(), alloc, table_flags)?;
        // SAFETY: l2_phys pointe sur un PD valide ou fraîchement alloué.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let entry = &mut pd[virt.p2_index()];
        if entry.is_present() && !entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        *entry = PageTableEntry::from_page_flags(Frame::containing(phys), huge_flags);
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
        let l4_entry = &mut pml4[virt.p4_index()];
        Self::upgrade_table_entry_permissions(l4_entry, new_flags);

        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // Huge page 1 GiB : cette fonction ne gère que les mappings 4 KiB
        if l3_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        let l3_entry = &mut pdpt[virt.p3_index()];
        Self::upgrade_table_entry_permissions(l3_entry, new_flags);

        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }
        // Huge page 2 MiB : idem
        if l2_entry.is_huge() {
            return Err(AllocError::InvalidParams);
        }
        let l2_entry = &mut pd[virt.p2_index()];
        Self::upgrade_table_entry_permissions(l2_entry, new_flags);

        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        let entry = &mut pt[virt.p1_index()];
        if !entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        let frame = entry.frame().ok_or(AllocError::InvalidParams)?;
        *entry = PageTableEntry::from_page_flags(frame, new_flags);
        Ok(())
    }

    /// Déplace une PTE feuille 4 KiB de `src` vers `dst` sans copier le frame.
    ///
    /// Les niveaux intermédiaires de destination sont alloués avant de modifier
    /// la source, donc un échec d'allocation ne perd pas le mapping d'origine.
    /// Retourne `Ok(false)` si `src` n'est pas résident (demand paging).
    pub fn move_leaf<A: FrameAllocatorForWalk>(
        &mut self,
        src: VirtAddr,
        dst: VirtAddr,
        alloc: &A,
    ) -> Result<bool, AllocError> {
        if src.as_u64() == dst.as_u64() {
            return Ok(true);
        }

        // DIAG-MVLEAF (temporaire) : un move_leaf (mremap zéro-copie / read zéro-copie)
        // qui vise la région pile (≥0x7fff...) REMAPPE une page de pile vers un autre
        // frame → écrase les return addresses → saut NULL.
        if dst.as_u64() >= 0x7fff_0000_0000 || src.as_u64() >= 0x7fff_0000_0000 {
            let out = crate::arch::x86_64::terminal::debug_write;
            let hexd = b"0123456789abcdef";
            let hx = |v: u64| {
                let mut b = [0u8; 12];
                let mut i = 0;
                while i < 12 {
                    b[i] = hexd[((v >> ((11 - i) * 4)) & 0xf) as usize];
                    i += 1;
                }
                out(&b);
            };
            out(b"<MVLEAF src=");
            hx(src.as_u64());
            out(b" dst=");
            hx(dst.as_u64());
            out(b">");
        }

        let Some(src_entry_ptr) = self.leaf_entry_ptr(src) else {
            return Ok(false);
        };
        // SAFETY: le pointeur vient de leaf_entry_ptr() et pointe vers une PTE 4 KiB.
        let old_entry = unsafe { *src_entry_ptr };
        if !old_entry.is_present() {
            return Ok(false);
        }

        let page_flags = old_entry.to_page_flags();

        // SAFETY: pml4_phys est une PML4 valide.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };

        let l3_phys = self.ensure_table(pml4, dst.p4_index(), alloc, page_flags)?;
        // SAFETY: l3_phys pointe vers une table valide.
        let pdpt = unsafe { phys_to_table_mut(l3_phys) };

        let l2_phys = self.ensure_table(pdpt, dst.p3_index(), alloc, page_flags)?;
        // SAFETY: l2_phys pointe vers une table valide.
        let pd = unsafe { phys_to_table_mut(l2_phys) };

        let l1_phys = self.ensure_table(pd, dst.p2_index(), alloc, page_flags)?;
        // SAFETY: l1_phys pointe vers une table valide.
        let pt = unsafe { phys_to_table_mut(l1_phys) };

        let dst_entry = &mut pt[dst.p1_index()];
        if dst_entry.is_present() {
            return Err(AllocError::InvalidParams);
        }

        *dst_entry = old_entry;
        // SAFETY: src_entry_ptr pointe toujours vers la PTE source; les tables
        // intermédiaires restent vivantes pendant toute la vie de l'AS.
        unsafe {
            (*src_entry_ptr).clear();
        }
        Ok(true)
    }

    /// Lit la valeur brute de la PTE feuille 4 KiB pour `virt`.
    /// Retourne `0` si la page n'est pas mappée ou si une huge page est rencontrée.
    pub fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let Some(entry_ptr) = self.leaf_entry_ptr(virt) else {
            return 0;
        };
        // SAFETY: `entry_ptr` pointe vers une PTE 4 KiB valide tant que la table existe.
        unsafe { (*entry_ptr).raw() }
    }

    /// Effectue un compare/exchange atomique sur une PTE feuille 4 KiB.
    ///
    /// Retourne `Err(actual_raw)` si l'entrée n'est plus égale à `expected`,
    /// ou `Err(0)` si aucune PTE feuille n'existe pour cette adresse.
    pub unsafe fn compare_exchange_leaf_raw(
        &self,
        virt: VirtAddr,
        expected: u64,
        new: u64,
    ) -> Result<(), u64> {
        let Some(entry_ptr) = self.leaf_entry_ptr(virt) else {
            return Err(0);
        };

        let atomic_ptr = entry_ptr.cast::<AtomicU64>();
        match (*atomic_ptr).compare_exchange(expected, new, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(()),
            Err(actual) => Err(actual),
        }
    }

    // ─── helper : s'assure qu'un niveau intermédiaire existe ─────────────────

    fn leaf_entry_ptr(&self, virt: VirtAddr) -> Option<*mut PageTableEntry> {
        // SAFETY: `pml4_phys` référence une hiérarchie valide pendant la vie de l'AS.
        let pml4 = unsafe { phys_to_table_mut(self.pml4_phys) };
        let l4_entry = pml4[virt.p4_index()];
        if !l4_entry.is_present() {
            return None;
        }

        // SAFETY: l'entrée présente pointe vers un PDPT valide.
        let pdpt = unsafe { phys_to_table_mut(l4_entry.phys_addr()) };
        let l3_entry = pdpt[virt.p3_index()];
        if !l3_entry.is_present() || l3_entry.is_huge() {
            return None;
        }

        // SAFETY: l'entrée présente pointe vers un PD valide.
        let pd = unsafe { phys_to_table_mut(l3_entry.phys_addr()) };
        let l2_entry = pd[virt.p2_index()];
        if !l2_entry.is_present() || l2_entry.is_huge() {
            return None;
        }

        // SAFETY: l'entrée présente pointe vers un PT valide.
        let pt = unsafe { phys_to_table_mut(l2_entry.phys_addr()) };
        Some((&mut pt[virt.p1_index()]) as *mut PageTableEntry)
    }

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
            if flags.contains(PageFlags::WRITABLE) {
                entry.set_flag(PageTableEntry::FLAG_WRITABLE);
            }
            if flags.contains(PageFlags::USER) {
                entry.set_flag(PageTableEntry::FLAG_USER);
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

    #[inline]
    fn upgrade_table_entry_permissions(entry: &mut PageTableEntry, leaf_flags: PageFlags) {
        // Les bits U/S et R/W des niveaux intermediaires restreignent toutes
        // les feuilles du sous-arbre. La feuille garde la politique fine
        // read-only/NX; les tables doivent seulement ne pas bloquer une page
        // legitime user ou writable plus bas.
        if leaf_flags.contains(PageFlags::WRITABLE) {
            entry.set_flag(PageTableEntry::FLAG_WRITABLE);
        }
        if leaf_flags.contains(PageFlags::USER) {
            entry.set_flag(PageTableEntry::FLAG_USER);
        }
    }
}
