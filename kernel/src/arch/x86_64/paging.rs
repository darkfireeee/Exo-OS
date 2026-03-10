//! # arch/x86_64/paging.rs — Page tables x86_64 (4-niveaux, CR3)
//!
//! Gère les page tables PML4 → PDPT → PD → PT pour x86_64.
//! Supporte KPTI (tables scindées user/kernel) via `spectre/kpti.rs`.
//!
//! ## Hiérarchie d'adressage
//! ```
//! Virtual address (48 bits) :
//!   [47:39] → PML4 index (512 entrées)
//!   [38:30] → PDPT index (512 entrées)
//!   [29:21] → PD index   (512 entrées)
//!   [20:12] → PT index   (512 entrées)
//!   [11:0]  → Page offset (4 KiB)
//! ```
//!
//! ## Flags de page
//! Conforme aux bits de l'entrée PT x86_64


use core::sync::atomic::{AtomicUsize, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

pub const PAGE_SIZE:      usize = 4096;
pub const HUGE_PAGE_2M:   usize = 2 * 1024 * 1024;
pub const HUGE_PAGE_1G:   usize = 1 * 1024 * 1024 * 1024;

pub const PAGE_TABLE_ENTRIES: usize = 512;

/// Masque pour l'adresse physique dans une entrée PTE
pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Masque bits [11:0] (flags)
pub const PTE_FLAGS_MASK: u64 = 0x0FFF;

// ── Flags PTE ─────────────────────────────────────────────────────────────────

/// Page présente
pub const PTE_PRESENT:    u64 = 1 << 0;
/// Page accessible en lecture/écriture (sinon read-only)
pub const PTE_WRITABLE:   u64 = 1 << 1;
/// Page accessible depuis le mode utilisateur (Ring 3)
pub const PTE_USER:       u64 = 1 << 2;
/// Write-Through caching
pub const PTE_WRITE_THROUGH: u64 = 1 << 3;
/// Cache Disabled
pub const PTE_CACHE_DISABLE: u64 = 1 << 4;
/// Page accédée (mis par le CPU lors d'un accès)
pub const PTE_ACCESSED:   u64 = 1 << 5;
/// Page dirty (mis par le CPU lors d'une écriture)
pub const PTE_DIRTY:      u64 = 1 << 6;
/// Huge page (dans PD = 2 MiB, dans PDPT = 1 GiB)
pub const PTE_HUGE:       u64 = 1 << 7;
/// Page globale (non flush sur CR3 switch si PGE actif)
pub const PTE_GLOBAL:     u64 = 1 << 8;
/// Bit 9 : utilisé par Exo-OS pour CoW pending
pub const PTE_COW:        u64 = 1 << 9;
/// Bit 10 : utilisé pour shared memory pinned
pub const PTE_SHM_PINNED: u64 = 1 << 10;
/// NX/XD bit — Non-Executable (bit 63)
pub const PTE_NO_EXEC:    u64 = 1 << 63;

/// Flags pour une page kernel normale (RW, global, no-exec data)
pub const PAGE_FLAGS_KERNEL_RW: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_GLOBAL | PTE_NO_EXEC;

/// Flags pour du code kernel (RX, global)
pub const PAGE_FLAGS_KERNEL_RX: u64 = PTE_PRESENT | PTE_GLOBAL;

/// Flags pour une page kernel read-only
pub const PAGE_FLAGS_KERNEL_RO: u64 = PTE_PRESENT | PTE_GLOBAL | PTE_NO_EXEC;

/// Flags pour une page userspace (RW, user)
pub const PAGE_FLAGS_USER_RW:   u64 = PTE_PRESENT | PTE_WRITABLE | PTE_USER | PTE_NO_EXEC;

/// Flags pour du code userspace (RX, user)
pub const PAGE_FLAGS_USER_RX:   u64 = PTE_PRESENT | PTE_USER;

/// Flags pour une page CoW (read-only pending copy)
pub const PAGE_FLAGS_USER_COW:  u64 = PTE_PRESENT | PTE_USER | PTE_COW | PTE_NO_EXEC;

/// Flags MMIO (pas de cache, RW, kernel)
pub const PAGE_FLAGS_MMIO:      u64 = PTE_PRESENT | PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXEC;

// ── Entrée de page table (PTE) ────────────────────────────────────────────────

/// Entrée de page table 64 bits
#[derive(Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Entrée vide (non présente)
    pub const fn empty() -> Self { Self(0) }

    /// Construit une entrée avec une adresse physique et des flags
    pub const fn new(phys_addr: u64, flags: u64) -> Self {
        Self((phys_addr & PTE_ADDR_MASK) | (flags & !PTE_ADDR_MASK))
    }

    /// Retourne l'adresse physique (sans flags)
    #[inline(always)]
    pub fn phys_addr(&self) -> u64 { self.0 & PTE_ADDR_MASK }

    /// Retourne les flags
    #[inline(always)]
    pub fn flags(&self) -> u64 { self.0 & !PTE_ADDR_MASK }

    /// Page présente ?
    #[inline(always)] pub fn is_present(&self)  -> bool { self.0 & PTE_PRESENT  != 0 }
    /// Page writable ?
    #[inline(always)] pub fn is_writable(&self) -> bool { self.0 & PTE_WRITABLE != 0 }
    /// Page user-accessible ?
    #[inline(always)] pub fn is_user(&self)     -> bool { self.0 & PTE_USER     != 0 }
    /// Huge page ?
    #[inline(always)] pub fn is_huge(&self)     -> bool { self.0 & PTE_HUGE     != 0 }
    /// No-execute ?
    #[inline(always)] pub fn is_no_exec(&self)  -> bool { self.0 & PTE_NO_EXEC  != 0 }
    /// CoW pending ?
    #[inline(always)] pub fn is_cow(&self)      -> bool { self.0 & PTE_COW      != 0 }
    /// SHM pinned ?
    #[inline(always)] pub fn is_shm_pinned(&self) -> bool { self.0 & PTE_SHM_PINNED != 0 }

    /// Ajoute les flags donnés
    #[inline(always)]
    pub fn set_flags(&mut self, flags: u64) { self.0 |= flags; }

    /// Efface les flags donnés
    #[inline(always)]
    pub fn clear_flags(&mut self, flags: u64) { self.0 &= !flags; }

    /// Valeur brute
    #[inline(always)]
    pub fn raw(&self) -> u64 { self.0 }
}

// ── Structure Page Table ──────────────────────────────────────────────────────

/// Une table de pages (512 × 8 bytes = 4 KiB, alignée 4 KiB)
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self { entries: [PageTableEntry::empty(); PAGE_TABLE_ENTRIES] }
    }

    /// Retourne l'entrée à l'index donné
    #[inline(always)]
    pub fn entry(&self, idx: usize) -> &PageTableEntry {
        &self.entries[idx & 0x1FF]
    }

    /// Retourne l'entrée mutable à l'index donné
    #[inline(always)]
    pub fn entry_mut(&mut self, idx: usize) -> &mut PageTableEntry {
        &mut self.entries[idx & 0x1FF]
    }

    /// Efface toutes les entrées (non présentes)
    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() { *e = PageTableEntry::empty(); }
    }
}

// ── Décomposition d'adresse virtuelle ────────────────────────────────────────

/// Indices dans les tables de pages pour une adresse virtuelle
#[derive(Debug, Clone, Copy)]
pub struct VirtAddrIndices {
    pub pml4_idx: usize,
    pub pdpt_idx: usize,
    pub pd_idx:   usize,
    pub pt_idx:   usize,
    pub offset:   usize,
}

/// Décompose une adresse virtuelle en indices de page tables
#[inline(always)]
pub fn decompose_virt_addr(va: u64) -> VirtAddrIndices {
    VirtAddrIndices {
        pml4_idx: ((va >> 39) & 0x1FF) as usize,
        pdpt_idx: ((va >> 30) & 0x1FF) as usize,
        pd_idx:   ((va >> 21) & 0x1FF) as usize,
        pt_idx:   ((va >> 12) & 0x1FF) as usize,
        offset:    (va & 0xFFF) as usize,
    }
}

/// Construit une adresse virtuelle depuis des indices
#[inline(always)]
pub fn compose_virt_addr(pml4: usize, pdpt: usize, pd: usize, pt: usize, off: usize) -> u64 {
    let raw = ((pml4 & 0x1FF) as u64) << 39
            | ((pdpt & 0x1FF) as u64) << 30
            | ((pd   & 0x1FF) as u64) << 21
            | ((pt   & 0x1FF) as u64) << 12
            | (off as u64 & 0xFFF);
    // Sign-extend bit 47 pour les adresses kernel
    if raw & (1u64 << 47) != 0 {
        raw | 0xFFFF_0000_0000_0000
    } else {
        raw
    }
}

// ── PML4 noyau statique (table racine kernel) ─────────────────────────────────

/// PML4 kernel statique (utilisé jusqu'à la fin du boot)
#[allow(dead_code)]
static mut KERNEL_PML4: PageTable = PageTable::empty();

/// Mappage brut : installe une entrée dans la hiérarchie existante
///
/// # SAFETY
/// - `pml4` doit pointer vers la PML4 active ou en cours de construction
/// - `phys_page` et les tables intermédiaires doivent être des frames valides
/// - L'appelant garantit l'absence de race sur les tables de pages
pub unsafe fn map_4k_page(
    pml4:      *mut PageTable,
    virt_addr: u64,
    phys_addr: u64,
    flags:     u64,
    alloc_page: impl Fn() -> Option<u64>,
) -> Result<(), PageTableError> {
    let idx = decompose_virt_addr(virt_addr);

    // PML4 → PDPT
    // SAFETY: `pml4` est un pointeur valide passé par l'appelant (unsafe fn) vers la
    // table PML4 active. Alignement 4 KiB garanti. Accès exclusif garanti par l'appelant.
    let pml4 = unsafe { &mut *pml4 };
    let pdpt = get_or_create_subtable(pml4, idx.pml4_idx, &alloc_page)?;

    // PDPT → PD
    let pd = get_or_create_subtable(pdpt, idx.pdpt_idx, &alloc_page)?;

    // PD → PT
    let pt = get_or_create_subtable(pd, idx.pd_idx, &alloc_page)?;

    // PT → page physique
    if pt.entry(idx.pt_idx).is_present() {
        return Err(PageTableError::AlreadyMapped);
    }
    *pt.entry_mut(idx.pt_idx) = PageTableEntry::new(phys_addr, flags);

    PAGE_MAP_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Démappage d'une page 4 KiB
///
/// # SAFETY
/// `pml4` est valide et l'adresse est mappée.
pub unsafe fn unmap_4k_page(
    pml4:      *mut PageTable,
    virt_addr: u64,
) -> Option<u64> {
    let idx  = decompose_virt_addr(virt_addr);
    // SAFETY: `pml4` est un pointeur valide (unsafe fn), aligné 4 KiB, accès exclusif.
    let pml4 = unsafe { &mut *pml4 };

    let e_pml4 = pml4.entry(idx.pml4_idx);
    if !e_pml4.is_present() { return None; }

    // SAFETY: e_pml4 est présent — phys_addr() pointe vers une PageTable valide, alignée 4 KiB.
    let pdpt = unsafe { &mut *(e_pml4.phys_addr() as *mut PageTable) };
    let e_pdpt = pdpt.entry(idx.pdpt_idx);
    if !e_pdpt.is_present() { return None; }

    // SAFETY: e_pdpt est présent — même invariant que ci-dessus.
    let pd = unsafe { &mut *(e_pdpt.phys_addr() as *mut PageTable) };
    let e_pd = pd.entry(idx.pd_idx);
    if !e_pd.is_present() || e_pd.is_huge() { return None; }

    // SAFETY: e_pd est présent et non-huge — même invariant.
    let pt = unsafe { &mut *(e_pd.phys_addr() as *mut PageTable) };
    let entry = pt.entry_mut(idx.pt_idx);
    if !entry.is_present() { return None; }

    let phys = entry.phys_addr();
    *entry = PageTableEntry::empty();

    // Invalider TLB localement
    super::invlpg(virt_addr);

    PAGE_UNMAP_COUNT.fetch_add(1, Ordering::Relaxed);
    Some(phys)
}

/// Traduit une adresse virtuelle en physique (page walk)
///
/// # SAFETY
/// `pml4` est valide. La hiérarchie ne doit pas être modifiée pendant le walk.
pub unsafe fn translate_virt(pml4: *const PageTable, virt_addr: u64) -> Option<u64> {
    let idx = decompose_virt_addr(virt_addr);
    // SAFETY: `pml4` est un pointeur valide passé par l'appelant (unsafe fn), aligné 4 KiB.
    // La hiérarchie de tables n'est pas modifiée pendant ce page walk.
    let pml4 = unsafe { &*pml4 };

    let e_pml4 = pml4.entry(idx.pml4_idx);
    if !e_pml4.is_present() { return None; }

    // SAFETY: e_pml4 est présent — phys_addr() est une PageTable valide, alignée 4 KiB.
    let pdpt = unsafe { &*(e_pml4.phys_addr() as *const PageTable) };
    let e_pdpt = pdpt.entry(idx.pdpt_idx);
    if !e_pdpt.is_present() { return None; }

    // Huge page 1 GiB ?
    if e_pdpt.is_huge() {
        let base = e_pdpt.phys_addr() & !(HUGE_PAGE_1G as u64 - 1);
        let off  = virt_addr & (HUGE_PAGE_1G as u64 - 1);
        return Some(base | off);
    }

    // SAFETY: e_pdpt est présent et non-huge — phys_addr() est une PageTable valide.
    let pd = unsafe { &*(e_pdpt.phys_addr() as *const PageTable) };
    let e_pd = pd.entry(idx.pd_idx);
    if !e_pd.is_present() { return None; }

    // Huge page 2 MiB ?
    if e_pd.is_huge() {
        let base = e_pd.phys_addr() & !(HUGE_PAGE_2M as u64 - 1);
        let off  = virt_addr & (HUGE_PAGE_2M as u64 - 1);
        return Some(base | off);
    }

    // SAFETY: e_pd est présent et non-huge — phys_addr() est une PageTable valide.
    let pt = unsafe { &*(e_pd.phys_addr() as *const PageTable) };
    let e_pt = pt.entry(idx.pt_idx);
    if !e_pt.is_present() { return None; }

    Some(e_pt.phys_addr() | idx.offset as u64)
}

// ── Helpers internes ──────────────────────────────────────────────────────────

fn get_or_create_subtable<'a>(
    parent:     &'a mut PageTable,
    idx:        usize,
    alloc_page: &impl Fn() -> Option<u64>,
) -> Result<&'a mut PageTable, PageTableError> {
    let entry = parent.entry_mut(idx);
    if !entry.is_present() {
        let phys = alloc_page().ok_or(PageTableError::OutOfMemory)?;
        *entry = PageTableEntry::new(phys, PTE_PRESENT | PTE_WRITABLE);

        // Initialiser la sous-table à zéro
        // SAFETY: phys est une frame fraîchement allouée — pas de contenu préalable
        unsafe {
            let ptr = phys as *mut PageTable;
            (*ptr).clear();
        }
    }
    // SAFETY: l'entrée est présente et pointe vers une PageTable valide
    Ok(unsafe { &mut *(entry.phys_addr() as *mut PageTable) })
}

// ── CR3 et TLB ───────────────────────────────────────────────────────────────

/// Charge le CR3 (switch page tables)
///
/// # SAFETY
/// `pml4_phys` doit pointer vers une PML4 valide alignée 4 KiB.
/// L'adresse de retour doit être mappée dans la nouvelle table.
#[inline(always)]
pub unsafe fn switch_page_table(pml4_phys: u64) {
    // SAFETY: délégué à l'appelant
    unsafe { super::write_cr3(pml4_phys); }
}

/// Flush complet du TLB (toutes les entrées non-globales)
#[inline(always)]
pub fn flush_tlb() {
    let cr3 = super::read_cr3();
    // SAFETY: re-écriture de la même valeur = flush TLB standard
    unsafe { super::write_cr3(cr3); }
}

/// Flush TLB pour une adresse virtuelle
#[inline(always)]
pub fn flush_tlb_page(virt: u64) {
    super::invlpg(virt);
}

/// Flush TLB global complet (inclut les pages globales, reset CR4.PGE)
pub fn flush_tlb_global() {
    let cr4 = super::read_cr4();
    const CR4_PGE: u64 = 1 << 7;
    // SAFETY: toggle CR4.PGE est la méthode documentée pour flush les pages globales
    unsafe {
        super::write_cr4(cr4 & !CR4_PGE);
        super::write_cr4(cr4 | CR4_PGE);
    }
}

// ── Erreurs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableError {
    AlreadyMapped,
    OutOfMemory,
    InvalidAlignment,
    NotMapped,
    HugePageConflict,
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static PAGE_MAP_COUNT:    AtomicUsize = AtomicUsize::new(0);
static PAGE_UNMAP_COUNT:  AtomicUsize = AtomicUsize::new(0);
static PAGE_FAULT_COUNT:  AtomicUsize = AtomicUsize::new(0);
static TLB_SHOOTDOWN_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn page_map_count()    -> usize { PAGE_MAP_COUNT.load(Ordering::Relaxed) }
pub fn page_unmap_count()  -> usize { PAGE_UNMAP_COUNT.load(Ordering::Relaxed) }
pub fn page_fault_count()  -> usize { PAGE_FAULT_COUNT.load(Ordering::Relaxed) }
pub fn inc_page_fault()              { PAGE_FAULT_COUNT.fetch_add(1, Ordering::Relaxed); }
pub fn inc_tlb_shootdown()           { TLB_SHOOTDOWN_COUNT.fetch_add(1, Ordering::Relaxed); }
pub fn tlb_shootdown_count() -> usize { TLB_SHOOTDOWN_COUNT.load(Ordering::Relaxed) }
