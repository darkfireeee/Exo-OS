//! Gestion des tables de pages pour la mémoire virtuelle
//! 
//! Ce module fournit des abstractions pour la gestion des tables de pages,
//! permettant de mapper des pages virtuelles à des frames physiques.

use core::ops::Range;
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    },
    PhysAddr, VirtAddr,
};
use crate::memory::{map_page, identity_map, PHYS_MEMORY_OFFSET};

/// Structure représentant un gestionnaire de tables de pages
pub struct PageTableManager {
    /// Table de pages offset pour la mémoire virtuelle
    page_table: OffsetPageTable<'static>,
}

impl PageTableManager {
    /// Crée un nouveau gestionnaire de tables de pages
    /// 
    /// # Arguments
    /// 
    /// * `frame_allocator` - Référence mutable à l'allocateur de frames
    /// 
    /// # Returns
    /// 
    /// Une nouvelle instance de `PageTableManager`
    pub fn new(frame_allocator: &mut impl FrameAllocator<Size4KiB>) -> Self {
        let page_table = crate::memory::init_page_tables(frame_allocator);
        Self { page_table }
    }
    
    /// Mappe une plage de pages à une plage de frames avec les flags spécifiés
    /// 
    /// # Arguments
    /// 
    /// * `frame_allocator` - Référence mutable à l'allocateur de frames
    /// * `page_range` - Plage de pages à mapper
    /// * `frame_range` - Plage de frames à mapper
    /// * `flags` - Flags des pages
    /// 
    /// # Returns
    /// 
    /// `Ok(())` si le mapping a réussi, `Err` sinon
    pub fn map_range(
        &mut self,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
        page_range: Range<Page<Size4KiB>>,
        frame_range: Range<PhysFrame<Size4KiB>>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        if page_range.end.start_address() - page_range.start.start_address() != 
           frame_range.end.start_address() - frame_range.start.start_address() {
            return Err("page range and frame range have different sizes");
        }
        
        for (page, frame) in page_range.zip(frame_range) {
            map_page(&mut self.page_table, frame_allocator, page, flags)?;
        }
        
        Ok(())
    }
    
    /// Mappe une page à une frame avec les flags spécifiés
    /// 
    /// # Arguments
    /// 
    /// * `frame_allocator` - Référence mutable à l'allocateur de frames
    /// * `page` - Page à mapper
    /// * `frame` - Frame à mapper
    /// * `flags` - Flags de la page
    /// 
    /// # Returns
    /// 
    /// `Ok(())` si le mapping a réussi, `Err` sinon
    pub fn map_page_to_frame(
        &mut self,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        use x86_64::structures::paging::Mapper;
        
        unsafe {
            self.page_table
                .map_to(page, frame, flags, frame_allocator)
                .map(|_| ()).map_err(|_| "map_to failed")
        }
    }
    
    /// Crée un mapping identity pour une plage de frames
    /// 
    /// # Arguments
    /// 
    /// * `frame_allocator` - Référence mutable à l'allocateur de frames
    /// * `frame_range` - Plage de frames à mapper
    /// * `flags` - Flags des pages
    /// 
    /// # Returns
    /// 
    /// `Ok(())` si le mapping a réussi, `Err` sinon
    pub fn identity_map_range(
        &mut self,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
        frame_range: Range<PhysFrame<Size4KiB>>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        use x86_64::structures::paging::PageRangeInclusive;
        
        let start_frame = frame_range.start;
        let end_frame = frame_range.end.previous_frame();
        
        identity_map(&mut self.page_table, frame_allocator, 
                    PhysFrame::range_inclusive(start_frame, end_frame), flags)
    }
    
    /// Alloue et mappe une plage de pages
    /// 
    /// # Arguments
    /// 
    /// * `frame_allocator` - Référence mutable à l'allocateur de frames
    /// * `page_range` - Plage de pages à allouer et mapper
    /// * `flags` - Flags des pages
    /// 
    /// # Returns
    /// 
    /// `Ok(())` si l'allocation et le mapping ont réussi, `Err` sinon
    pub fn allocate_and_map_range(
        &mut self,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
        page_range: Range<Page<Size4KiB>>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        for page in page_range {
            let frame = frame_allocator.allocate_frame().ok_or("out of memory")?;
            self.map_page_to_frame(frame_allocator, page, frame, flags)?;
        }
        
        Ok(())
    }
    
    /// Change les flags d'une page déjà mappée
    /// 
    /// # Arguments
    /// 
    /// * `page` - Page dont on veut changer les flags
    /// * `flags` - Nouveaux flags
    /// 
    /// # Returns
    /// 
    /// `Ok(())` si le changement a réussi, `Err` sinon
    pub fn update_flags(&mut self, page: Page<Size4KiB>, flags: PageTableFlags) -> Result<(), &'static str> {
        use x86_64::structures::paging::Mapper;
        
        self.page_table
            .update_flags(page, flags)
            .map(|_| ()).map_err(|_| "update_flags failed")
    }
    
    /// Traduit une adresse virtuelle en adresse physique
    /// 
    /// # Arguments
    /// 
    /// * `addr` - Adresse virtuelle à traduire
    /// 
    /// # Returns
    /// 
    /// L'adresse physique correspondante, ou `None` si la page n'est pas mappée
    pub fn translate_addr(&self, addr: VirtAddr) -> Option<PhysAddr> {
        self.page_table.translate_addr(addr)
    }
    
    /// Vérifie si une page est mappée
    /// 
    /// # Arguments
    /// 
    /// * `page` - Page à vérifier
    /// 
    /// # Returns
    /// 
    /// `true` si la page est mappée, `false` sinon
    pub fn is_page_mapped(&self, page: Page<Size4KiB>) -> bool {
        self.translate_addr(page.start_address()).is_some()
    }
    
    /// Retourne une référence à la table de pages offset
    pub fn page_table(&self) -> &OffsetPageTable {
        &self.page_table
    }
    
    /// Retourne une référence mutable à la table de pages offset
    pub fn page_table_mut(&mut self) -> &mut OffsetPageTable {
        &mut self.page_table
    }
}

/// Active les pages de grande taille (2 MiB/1 GiB) si supporté
pub fn enable_large_pages() {
    use x86_64::registers::control::{Cr4, Cr4Flags};
    
    if x86_64::cpuid::CpuId::new()
        .get_feature_info()
        .map_or(false, |finfo| finfo.has_page_size_extension())
    {
        unsafe { Cr4::update(|cr4| *cr4 |= Cr4Flags::PAGE_SIZE_EXTENSION) };
    }
}

/// Active la protection contre l'exécution de données (DEP/NX)
pub fn enable_nx_bit() {
    use x86_64::registers::control::{Efer, EferFlags};
    
    unsafe { Efer::update(|efer| *efer |= EferFlags::NO_EXECUTE_ENABLE) };
}

/// Active l'écriture protégée (WP)
pub fn enable_write_protect() {
    use x86_64::registers::control::Cr0;
    
    unsafe { Cr0::update(|cr0| *cr0 |= Cr0Flags::WRITE_PROTECT) };
}

/// Invalide une page dans le TLB
/// 
/// # Arguments
/// 
/// * `addr` - Adresse de la page à invalider
pub fn invalidate_page(addr: VirtAddr) {
    unsafe {
        x86_64::instructions::tlb::invalidate(addr);
    }
}

/// Invalide toutes les pages dans le TLB
pub fn invalidate_all_pages() {
    unsafe {
        x86_64::instructions::tlb::flush_all();
    }
}