// kernel/src/memory/dma/iommu/page_table.rs
//
// Tables de pages IOMMU (Intel VT-d / AMD-Vi format unifié côté abstraction).
// L'implémentation concrète hardware est dans intel_vtd.rs / amd_iommu.rs.
// Ce fichier fournit la logique d'arbre de pages portable.
//
// Format : 4 niveaux (L4/L3/L2/L1), 512 entrées par niveau, 4096 octets par table.
// COUCHE 0 — aucune dépendance externe.

use crate::memory::core::types::PhysAddr;
use crate::memory::core::address::phys_to_virt;
use crate::memory::dma::core::types::{IovaAddr, DmaError};

// ─────────────────────────────────────────────────────────────────────────────
// ENTRÉE DE TABLE IOMMU
// ─────────────────────────────────────────────────────────────────────────────

/// Flags d'une entrée de table IOMMU.
pub mod iommu_entry_flags {
    /// Entrée présente.
    pub const PRESENT:      u64 = 1 << 0;
    /// Lecture autorisée (device → mémoire).
    pub const READ:         u64 = 1 << 1;
    /// Écriture autorisée (mémoire → device).
    pub const WRITE:        u64 = 1 << 2;
    /// Cache ignoré (device coherency bypass).
    pub const SNOOP:        u64 = 1 << 3;
    /// Huge page (2MiB).
    pub const HUGE:         u64 = 1 << 7;
    /// Accessed (matériel).
    pub const ACCESSED:     u64 = 1 << 8;
    /// Dirty (matériel).
    pub const DIRTY:        u64 = 1 << 9;
    /// Masque de l'adresse physique du frame pointé.
    pub const PHYS_MASK:    u64 = 0x000F_FFFF_FFFF_F000;
}

/// Une entrée de table IOMMU (64 bits).
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct IommuEntry(u64);

impl IommuEntry {
    pub const EMPTY: Self = IommuEntry(0);

    #[inline]
    pub fn is_present(self) -> bool {
        self.0 & iommu_entry_flags::PRESENT != 0
    }
    #[inline]
    pub fn is_huge(self) -> bool {
        self.0 & iommu_entry_flags::HUGE != 0
    }
    #[inline]
    pub fn phys(self) -> PhysAddr {
        PhysAddr::new(self.0 & iommu_entry_flags::PHYS_MASK)
    }
    /// Construit une entrée pointant sur une table de niveau inférieur.
    #[inline]
    pub fn table_entry(phys: PhysAddr) -> Self {
        IommuEntry(phys.as_u64() | iommu_entry_flags::PRESENT |
                   iommu_entry_flags::READ | iommu_entry_flags::WRITE)
    }
    /// Construit une entrée feuille (page 4KiB).
    #[inline]
    pub fn leaf_entry(phys: PhysAddr, read: bool, write: bool) -> Self {
        let mut flags = iommu_entry_flags::PRESENT;
        if read  { flags |= iommu_entry_flags::READ; }
        if write { flags |= iommu_entry_flags::WRITE; }
        IommuEntry(phys.as_u64() | flags | iommu_entry_flags::SNOOP)
    }
    #[inline]
    pub fn raw(self) -> u64 { self.0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE PAGES IOMMU (512 ENTRÉES, 4KiB)
// ─────────────────────────────────────────────────────────────────────────────

/// Une table de pages IOMMU (4096 octets, 512 entrées 64-bit).
#[repr(C, align(4096))]
pub struct IommuPageTable {
    entries: [IommuEntry; 512],
}

impl IommuPageTable {
    /// Retourne l'adresse physique de cette table via le physmap.
    pub fn phys_addr(&self) -> PhysAddr {
        use crate::memory::core::address::virt_to_phys_physmap;
        let virt = crate::memory::core::types::VirtAddr::new(self as *const _ as u64);
        virt_to_phys_physmap(virt)
    }

    /// Index IOMMU de l'IOVA au niveau `level` (0=L1, 3=L4).
    #[inline]
    pub fn index(iova: IovaAddr, level: u8) -> usize {
        ((iova.as_u64() >> (12 + level as u64 * 9)) & 0x1FF) as usize
    }

    /// Retourne l'entrée à l'index `idx`.
    #[inline]
    pub fn entry(&self, idx: usize) -> IommuEntry {
        // SAFETY: idx < 512, lecture de mémoire valide.
        unsafe { core::ptr::read_volatile(&self.entries[idx]) }
    }

    /// Écrit l'entrée à l'index `idx` de manière atomique.
    #[inline]
    pub fn set_entry(&mut self, idx: usize, entry: IommuEntry) {
        // SAFETY: idx < 512, écriture dans la table mappée.
        unsafe { core::ptr::write_volatile(&mut self.entries[idx], entry); }
    }

    /// Zéro-remplit toute la table.
    pub fn zero(&mut self) {
        // SAFETY: La table est entièrement initialisée.
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 512);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MARCHEUR DE TABLE IOMMU
// ─────────────────────────────────────────────────────────────────────────────

/// Trait d'allocateur de frames pour les tables IOMMU (inversion de dépendance).
pub trait IommuFrameAlloc {
    fn alloc_table(&mut self) -> Option<PhysAddr>;
    fn free_table(&mut self, phys: PhysAddr);
}

/// Résultat d'un walk IOMMU.
pub enum IommuWalkResult {
    /// Mapping feuille trouvé.
    Leaf { phys: PhysAddr, huge: bool },
    /// Adresse non mappée.
    NotMapped,
    /// Erreur d'allocation lors d'un map.
    AllocError,
}

/// Configure un mapping IOVA → PhysAddr dans la table enracinée à `root`.
///
/// Crée les tables intermédiaires si nécessaire via `alloc`.
///
/// # Safety
/// `root` doit pointer une table de pages IOMMU valide et mappée dans le physmap.
pub unsafe fn iommu_map<A: IommuFrameAlloc>(
    root:  PhysAddr,
    iova:  IovaAddr,
    phys:  PhysAddr,
    read:  bool,
    write: bool,
    alloc: &mut A,
) -> Result<(), DmaError> {
    let virt_root = phys_to_virt(root);
    let l4 = &mut *(virt_root.as_u64() as *mut IommuPageTable);

    let l4_idx = IommuPageTable::index(iova, 3);
    let l3_phys = ensure_table(l4, l4_idx, alloc)?;

    let l3 = &mut *(phys_to_virt(l3_phys).as_u64() as *mut IommuPageTable);
    let l3_idx = IommuPageTable::index(iova, 2);
    let l2_phys = ensure_table(l3, l3_idx, alloc)?;

    let l2 = &mut *(phys_to_virt(l2_phys).as_u64() as *mut IommuPageTable);
    let l2_idx = IommuPageTable::index(iova, 1);
    let l1_phys = ensure_table(l2, l2_idx, alloc)?;

    let l1 = &mut *(phys_to_virt(l1_phys).as_u64() as *mut IommuPageTable);
    let l1_idx = IommuPageTable::index(iova, 0);
    l1.set_entry(l1_idx, IommuEntry::leaf_entry(phys, read, write));
    Ok(())
}

/// Supprime un mapping IOVA de la table.
///
/// # Safety
/// `root` doit pointer une table de pages IOMMU valide.
pub unsafe fn iommu_unmap(root: PhysAddr, iova: IovaAddr) -> Result<(), DmaError> {
    let l4 = &mut *(phys_to_virt(root).as_u64() as *mut IommuPageTable);
    let l4_idx = IommuPageTable::index(iova, 3);
    let e4 = l4.entry(l4_idx);
    if !e4.is_present() { return Err(DmaError::InvalidParams); }

    let l3 = &mut *(phys_to_virt(e4.phys()).as_u64() as *mut IommuPageTable);
    let l3_idx = IommuPageTable::index(iova, 2);
    let e3 = l3.entry(l3_idx);
    if !e3.is_present() { return Err(DmaError::InvalidParams); }

    let l2 = &mut *(phys_to_virt(e3.phys()).as_u64() as *mut IommuPageTable);
    let l2_idx = IommuPageTable::index(iova, 1);
    let e2 = l2.entry(l2_idx);
    if !e2.is_present() { return Err(DmaError::InvalidParams); }

    let l1 = &mut *(phys_to_virt(e2.phys()).as_u64() as *mut IommuPageTable);
    let l1_idx = IommuPageTable::index(iova, 0);
    l1.set_entry(l1_idx, IommuEntry::EMPTY);
    Ok(())
}

/// Parcourt la table en lecture seule pour `iova`.
///
/// # Safety
/// `root` doit pointer une table valide.
pub unsafe fn iommu_walk(root: PhysAddr, iova: IovaAddr) -> IommuWalkResult {
    let l4 = &*(phys_to_virt(root).as_u64() as *const IommuPageTable);
    let e4 = l4.entry(IommuPageTable::index(iova, 3));
    if !e4.is_present() { return IommuWalkResult::NotMapped; }

    let l3 = &*(phys_to_virt(e4.phys()).as_u64() as *const IommuPageTable);
    let e3 = l3.entry(IommuPageTable::index(iova, 2));
    if !e3.is_present() { return IommuWalkResult::NotMapped; }
    if e3.is_huge() { return IommuWalkResult::Leaf { phys: e3.phys(), huge: true }; }

    let l2 = &*(phys_to_virt(e3.phys()).as_u64() as *const IommuPageTable);
    let e2 = l2.entry(IommuPageTable::index(iova, 1));
    if !e2.is_present() { return IommuWalkResult::NotMapped; }

    let l1 = &*(phys_to_virt(e2.phys()).as_u64() as *const IommuPageTable);
    let e1 = l1.entry(IommuPageTable::index(iova, 0));
    if !e1.is_present() { return IommuWalkResult::NotMapped; }
    IommuWalkResult::Leaf { phys: e1.phys(), huge: false }
}

/// Assure qu'une table de niveau inférieur existe à l'entrée `idx` de `parent`.
/// La crée via `alloc` si absente.
unsafe fn ensure_table<A: IommuFrameAlloc>(
    parent: &mut IommuPageTable,
    idx:    usize,
    alloc:  &mut A,
) -> Result<PhysAddr, DmaError> {
    let entry = parent.entry(idx);
    if entry.is_present() {
        return Ok(entry.phys());
    }
    // Alloue une nouvelle table.
    let phys = alloc.alloc_table().ok_or(DmaError::OutOfMemory)?;
    // Zéro-remplit la nouvelle table.
    let table = &mut *(phys_to_virt(phys).as_u64() as *mut IommuPageTable);
    table.zero();
    parent.set_entry(idx, IommuEntry::table_entry(phys));
    Ok(phys)
}
