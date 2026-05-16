// kernel/src/memory/virtual/page_table/builder.rs
//
// Builder de table de pages — construction incrémentale d'une PML4.
// Interface fluente pour les mappings kernel et user au boot.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{
    AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr, HUGE_PAGE_SIZE, PAGE_SIZE,
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
        let flags = PageFlags::KERNEL_DATA;

        let page_size = PAGE_SIZE as u64;
        let huge_size = HUGE_PAGE_SIZE as u64;
        let mut offset = 0u64;
        while offset < phys_size {
            let remaining = phys_size - offset;
            let virt = VirtAddr::new(phys_map_base.as_u64() + offset);
            let phys = PhysAddr::new(offset);
            if remaining >= huge_size
                && (virt.as_u64() & (huge_size - 1)) == 0
                && phys.is_aligned(huge_size)
            {
                self.walker.map_huge_2m(virt, phys, flags, self.alloc)?;
                offset = offset.saturating_add(huge_size);
            } else {
                self.walker
                    .map(virt, Frame::containing(phys), flags, self.alloc)?;
                offset = offset.saturating_add(page_size);
            }
        }
        Ok(self)
    }

    /// Clone les entrées kernel (PML4[256..512]) depuis la PML4 active.
    /// Utilisé pour créer l'espace utilisateur qui partage le noyau.
    ///
    /// Le kernel actuel est encore linké et exécuté dans la fenêtre basse
    /// identity-map (à partir de 1 MiB). Les CR3 userspace doivent donc aussi
    /// contenir l'image noyau basse en pages supervisor, sinon le premier
    /// `context_switch_asm` faute immédiatement après `mov cr3`.
    ///
    /// SAFETY: Doit être appelé quand la PML4 active est complète et valide.
    pub unsafe fn copy_kernel_entries(&mut self) -> Result<&mut Self, AllocError> {
        let current_pml4_phys = read_cr3();
        let current_pml4 = phys_to_table_mut(current_pml4_phys);
        let new_pml4 = phys_to_table_mut(self.pml4_phys);
        // Entrées kernel : indices 256..512 (moitié haute de l'espace 48 bits)
        for i in 256..512 {
            new_pml4[i] = current_pml4[i];
        }

        self.remap_low_kernel_identity()?;
        Ok(self)
    }

    /// Réinstalle les mappings supervisor de l'image noyau basse dans cette PML4.
    ///
    /// Le noyau exécute encore ses chemins syscall/IRQ dans la fenêtre basse
    /// identity-map. Cette primitive est volontairement idempotente: elle
    /// réécrit les PTE noyau attendues sans toucher aux segments user qui sont
    /// placés hors de l'image noyau.
    pub fn remap_low_kernel_identity(&mut self) -> Result<&mut Self, AllocError> {
        self.map_low_kernel_identity()?;
        Ok(self)
    }

    #[cfg(target_os = "none")]
    fn map_low_kernel_identity(&mut self) -> Result<(), AllocError> {
        unsafe extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __data_start: u8;
            static __data_end: u8;
            static __bss_start: u8;
            static __bss_end: u8;
            static __kernel_end: u8;
        }

        let image_start = crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR;
        let text_start = (&raw const __text_start) as u64;
        let text_end = (&raw const __text_end) as u64;
        let rodata_start = (&raw const __rodata_start) as u64;
        let rodata_end = (&raw const __rodata_end) as u64;
        let data_start = (&raw const __data_start) as u64;
        let data_end = (&raw const __data_end) as u64;
        let bss_start = (&raw const __bss_start) as u64;
        let bss_end = (&raw const __bss_end) as u64;
        let kernel_end = (&raw const __kernel_end) as u64;

        let kernel_rxw = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::GLOBAL;
        let kernel_ro = PageFlags::PRESENT | PageFlags::GLOBAL | PageFlags::NO_EXECUTE;
        let text_end_page = Self::page_align_up(text_end);
        let rodata_end_page = Self::page_align_up(rodata_end);
        let data_end_page = Self::page_align_up(data_end);
        let bss_end_page = Self::page_align_up(bss_end);

        // Ne jamais poser temporairement NX sur .text quand ce CR3 est actif:
        // une IRQ/#PF pendant fork doit pouvoir exécuter ses handlers.
        self.map_identity_range(image_start, text_start, kernel_rxw)?;
        self.map_identity_range(text_start, text_end, PageFlags::KERNEL_CODE)?;
        self.map_identity_range(text_end_page, rodata_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(rodata_start, rodata_end, kernel_ro)?;
        self.map_identity_range(rodata_end_page, data_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(data_start, data_end, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(data_end_page, bss_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(bss_start, bss_end, PageFlags::KERNEL_DATA)?;

        if bss_end < kernel_end {
            self.map_identity_range(bss_end_page, kernel_end, PageFlags::KERNEL_DATA)?;
        }

        // Le noyau courant utilise encore les fenêtres MMIO basses héritées du
        // boot pour LAPIC/IOAPIC, et certains chemins timer restent actifs sous
        // CR3 userspace avant un éventuel switch KPTI complet.
        self.map_identity_range(0xFEC0_0000, 0xFEC0_1000, PageFlags::KERNEL_DMA)?;
        self.map_identity_range(0xFED0_0000, 0xFED0_1000, PageFlags::KERNEL_DMA)?;
        self.map_identity_range(0xFEE0_0000, 0xFEE0_1000, PageFlags::KERNEL_DMA)?;
        Ok(())
    }

    #[cfg(not(target_os = "none"))]
    fn map_low_kernel_identity(&mut self) -> Result<(), AllocError> {
        Ok(())
    }

    #[cfg(target_os = "none")]
    fn map_identity_range(
        &mut self,
        start: u64,
        end: u64,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        if end <= start {
            return Ok(());
        }

        let page_mask = PAGE_SIZE as u64 - 1;
        let mut addr = start & !page_mask;
        let end_aligned = end.saturating_add(page_mask) & !page_mask;
        while addr < end_aligned {
            let phys = PhysAddr::new(addr);
            self.map_page(VirtAddr::new(addr), Frame::containing(phys), flags)?;
            addr = addr.saturating_add(PAGE_SIZE as u64);
        }
        Ok(())
    }

    #[cfg(target_os = "none")]
    #[inline]
    fn page_align_up(addr: u64) -> u64 {
        let page_mask = PAGE_SIZE as u64 - 1;
        addr.saturating_add(page_mask) & !page_mask
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
