// kernel/src/memory/virtual/page_table/x86_64.rs
//
// Tables de pages x86_64 4 niveaux (PML4 → PDPT → PD → PT).
// Implémentation complète avec flags, entrées, et helpers d'accès.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::ops::{Index, IndexMut};

use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE, PHYS_MAP_BASE};

// ─────────────────────────────────────────────────────────────────────────────
// ENTRÉE DE TABLE DE PAGES (PageTableEntry)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'une table de pages x86_64 (64 bits).
///
/// Bits 0-11 : flags
/// Bits 12-51: adresse physique du frame (alignée sur 4 KiB)
/// Bits 52-62: flags supérieurs (disponibles)
/// Bit  63   : NX (No-Execute)
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Entrée nulle (non présente).
    pub const EMPTY: PageTableEntry = PageTableEntry(0);

    // ── Bits de flags ────────────────────────────────────────────────────────
    pub const FLAG_PRESENT: u64 = 1 << 0;
    pub const FLAG_WRITABLE: u64 = 1 << 1;
    pub const FLAG_USER: u64 = 1 << 2;
    pub const FLAG_WRITE_THROUGH: u64 = 1 << 3;
    pub const FLAG_NO_CACHE: u64 = 1 << 4;
    pub const FLAG_ACCESSED: u64 = 1 << 5;
    pub const FLAG_DIRTY: u64 = 1 << 6;
    pub const FLAG_HUGE_PAGE: u64 = 1 << 7; // PSE bit dans PD/PDPT
    pub const FLAG_GLOBAL: u64 = 1 << 8;
    pub const FLAG_COW: u64 = 1 << 9; // Disponible OS
    pub const FLAG_PINNED: u64 = 1 << 10; // Disponible OS
    pub const FLAG_NO_EXECUTE: u64 = 1 << 63;

    /// Masque des bits d'adresse physique.
    const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    #[inline]
    pub fn new(frame: Frame, flags: u64) -> Self {
        PageTableEntry(frame.start_address().as_u64() | flags)
    }

    #[inline]
    pub fn from_raw(raw: u64) -> Self {
        PageTableEntry(raw)
    }
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
    #[inline]
    pub fn is_present(self) -> bool {
        self.0 & Self::FLAG_PRESENT != 0
    }
    #[inline]
    pub fn is_writable(self) -> bool {
        self.0 & Self::FLAG_WRITABLE != 0
    }
    #[inline]
    pub fn is_user(self) -> bool {
        self.0 & Self::FLAG_USER != 0
    }
    #[inline]
    pub fn is_huge(self) -> bool {
        self.0 & Self::FLAG_HUGE_PAGE != 0
    }
    #[inline]
    pub fn is_global(self) -> bool {
        self.0 & Self::FLAG_GLOBAL != 0
    }
    #[inline]
    pub fn is_no_execute(self) -> bool {
        self.0 & Self::FLAG_NO_EXECUTE != 0
    }
    #[inline]
    pub fn is_cow(self) -> bool {
        self.0 & Self::FLAG_COW != 0
    }
    #[inline]
    pub fn is_pinned(self) -> bool {
        self.0 & Self::FLAG_PINNED != 0
    }
    #[inline]
    pub fn is_accessed(self) -> bool {
        self.0 & Self::FLAG_ACCESSED != 0
    }
    #[inline]
    pub fn is_dirty(self) -> bool {
        self.0 & Self::FLAG_DIRTY != 0
    }

    /// Adresse physique encodée dans cette entrée.
    #[inline]
    pub fn phys_addr(self) -> PhysAddr {
        PhysAddr::new(self.0 & Self::PHYS_MASK)
    }

    /// Frame physique pointé par cette entrée.
    #[inline]
    pub fn frame(self) -> Option<Frame> {
        if self.is_present() {
            Some(Frame::containing(self.phys_addr()))
        } else {
            None
        }
    }

    /// Active un flag.
    #[inline]
    pub fn set_flag(&mut self, flag: u64) {
        self.0 |= flag;
    }
    /// Désactive un flag.
    #[inline]
    pub fn clear_flag(&mut self, flag: u64) {
        self.0 &= !flag;
    }

    /// Efface complètement l'entrée.
    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    /// Convertit les PageFlags kernel en bits d'entrée x86_64.
    pub fn from_page_flags(frame: Frame, flags: PageFlags) -> Self {
        let mut raw = frame.start_address().as_u64();
        if flags.contains(PageFlags::PRESENT) {
            raw |= Self::FLAG_PRESENT;
        }
        if flags.contains(PageFlags::WRITABLE) {
            raw |= Self::FLAG_WRITABLE;
        }
        if flags.contains(PageFlags::USER) {
            raw |= Self::FLAG_USER;
        }
        if flags.contains(PageFlags::WRITE_THROUGH) {
            raw |= Self::FLAG_WRITE_THROUGH;
        }
        if flags.contains(PageFlags::NO_CACHE) {
            raw |= Self::FLAG_NO_CACHE;
        }
        if flags.contains(PageFlags::GLOBAL) {
            raw |= Self::FLAG_GLOBAL;
        }
        if flags.contains(PageFlags::NO_EXECUTE) {
            raw |= Self::FLAG_NO_EXECUTE;
        }
        if flags.contains(PageFlags::COW) {
            raw |= Self::FLAG_COW;
        }
        if flags.contains(PageFlags::PINNED) {
            raw |= Self::FLAG_PINNED;
        }
        PageTableEntry(raw)
    }

    /// Extrait les PageFlags depuis une entrée.
    pub fn to_page_flags(self) -> PageFlags {
        let mut f = PageFlags::EMPTY;
        if self.0 & Self::FLAG_PRESENT != 0 {
            f = f.set(PageFlags::PRESENT);
        }
        if self.0 & Self::FLAG_WRITABLE != 0 {
            f = f.set(PageFlags::WRITABLE);
        }
        if self.0 & Self::FLAG_USER != 0 {
            f = f.set(PageFlags::USER);
        }
        if self.0 & Self::FLAG_WRITE_THROUGH != 0 {
            f = f.set(PageFlags::WRITE_THROUGH);
        }
        if self.0 & Self::FLAG_NO_CACHE != 0 {
            f = f.set(PageFlags::NO_CACHE);
        }
        if self.0 & Self::FLAG_ACCESSED != 0 {
            f = f.set(PageFlags::ACCESSED);
        }
        if self.0 & Self::FLAG_DIRTY != 0 {
            f = f.set(PageFlags::DIRTY);
        }
        if self.0 & Self::FLAG_GLOBAL != 0 {
            f = f.set(PageFlags::GLOBAL);
        }
        if self.0 & Self::FLAG_NO_EXECUTE != 0 {
            f = f.set(PageFlags::NO_EXECUTE);
        }
        if self.0 & Self::FLAG_COW != 0 {
            f = f.set(PageFlags::COW);
        }
        if self.0 & Self::FLAG_PINNED != 0 {
            f = f.set(PageFlags::PINNED);
        }
        f
    }
}

impl core::fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PTE({:#018x} phys={:#x} flags={:?})",
            self.0,
            self.0 & Self::PHYS_MASK,
            self.to_page_flags()
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE PAGES (512 entrées)
// ─────────────────────────────────────────────────────────────────────────────

/// Une table de pages x86_64 (exactement une page = 4 KiB = 512 × 8 octets).
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Crée une table vide (toutes les entrées = 0).
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::EMPTY; 512],
        }
    }

    /// Efface toutes les entrées.
    pub fn zero(&mut self) {
        for e in &mut self.entries {
            e.clear();
        }
    }

    /// Efface les entrées user (indices 0..256).
    pub fn zero_user_half(&mut self) {
        for e in &mut self.entries[0..256] {
            e.clear();
        }
    }

    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, PageTableEntry> {
        self.entries.iter()
    }
    #[inline]
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, PageTableEntry> {
        self.entries.iter_mut()
    }
    #[inline]
    pub fn len(&self) -> usize {
        512
    }

    /// Adresse physique de cette table (via le physmap).
    pub fn phys_addr(&self) -> PhysAddr {
        // SAFETY: self est dans la mémoire physique mappée via PHYS_MAP_BASE.
        let virt = self as *const _ as u64;
        if virt >= PHYS_MAP_BASE.as_u64() {
            PhysAddr::new(virt - PHYS_MAP_BASE.as_u64())
        } else {
            // Fallback pour les tables dans le segment .bss kernel.
            PhysAddr::new(
                virt - crate::memory::core::layout::KERNEL_START.as_u64() + KERNEL_PHYS_BASE,
            )
        }
    }
}

/// Base physique du kernel (supposée 1 MiB pour x86_64 standard).
const KERNEL_PHYS_BASE: u64 = 0x0010_0000;

impl Index<usize> for PageTable {
    type Output = PageTableEntry;
    fn index(&self, i: usize) -> &Self::Output {
        &self.entries[i]
    }
}
impl IndexMut<usize> for PageTable {
    fn index_mut(&mut self, i: usize) -> &mut Self::Output {
        &mut self.entries[i]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NIVEAUX DE TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau d'une table de pages (4=PML4, 3=PDPT, 2=PD, 1=PT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PageTableLevel {
    /// Page Table (niveau 1).
    L1 = 1,
    /// Page Directory (niveau 2).
    L2 = 2,
    /// Page Directory Pointer Table (niveau 3).
    L3 = 3,
    /// PML4 (niveau 4).
    L4 = 4,
}

impl PageTableLevel {
    /// Retourne l'index dans la VirtAddr pour ce niveau.
    #[inline]
    pub fn index_of(self, addr: VirtAddr) -> usize {
        match self {
            PageTableLevel::L4 => addr.p4_index(),
            PageTableLevel::L3 => addr.p3_index(),
            PageTableLevel::L2 => addr.p2_index(),
            PageTableLevel::L1 => addr.p1_index(),
        }
    }

    /// Niveau inférieur.
    pub fn descend(self) -> Option<PageTableLevel> {
        match self {
            PageTableLevel::L4 => Some(PageTableLevel::L3),
            PageTableLevel::L3 => Some(PageTableLevel::L2),
            PageTableLevel::L2 => Some(PageTableLevel::L1),
            PageTableLevel::L1 => None,
        }
    }

    /// Taille de page mappée à ce niveau (huge page).
    pub fn page_size(self) -> usize {
        match self {
            PageTableLevel::L4 => 512 * 1024 * 1024 * 1024, // 512 GiB (non utilisé)
            PageTableLevel::L3 => 1024 * 1024 * 1024,       // 1 GiB
            PageTableLevel::L2 => 2 * 1024 * 1024,          // 2 MiB
            PageTableLevel::L1 => PAGE_SIZE,                // 4 KiB
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS : physmap → table de pages
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne un pointeur vers la table de pages physique via le physmap.
///
/// SAFETY: `phys` doit pointer sur une PageTable valide et le physmap doit
///         être initialisé (PHYS_MAP_BASE mappé).
#[inline]
pub unsafe fn phys_to_table(phys: PhysAddr) -> *mut PageTable {
    (PHYS_MAP_BASE.as_u64() + phys.as_u64()) as *mut PageTable
}

/// Retourne une référence vers la table de pages physique via le physmap.
///
/// SAFETY: Même conditions que phys_to_table. La durée de vie est liée au
///         physmap (statique).
#[inline]
pub unsafe fn phys_to_table_ref<'a>(phys: PhysAddr) -> &'a PageTable {
    &*(phys_to_table(phys))
}

/// Retourne une référence mutable vers la table de pages physique.
///
/// SAFETY: Même conditions que phys_to_table. Aucune autre référence
///         mutable ne doit exister simultanément.
#[inline]
pub unsafe fn phys_to_table_mut<'a>(phys: PhysAddr) -> &'a mut PageTable {
    &mut *(phys_to_table(phys))
}

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRE CR3
// ─────────────────────────────────────────────────────────────────────────────

/// Lit l'adresse physique de la PML4 active depuis CR3.
#[inline]
pub fn read_cr3() -> PhysAddr {
    let val: u64;
    // SAFETY: lecture de CR3, opération x86_64 standard au niveau ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) val, options(nomem, nostack));
    }
    PhysAddr::new(val & !0xFFF)
}

/// Charge une nouvelle PML4 dans CR3 (invalide tout le TLB global).
///
/// SAFETY: `pml4` doit pointer sur une PML4 valide. Des entrées invalides
///         provoqueront une #PF ou #GP immédiate.
#[inline]
pub unsafe fn write_cr3(pml4: PhysAddr) {
    let pcid_bits = read_cr3().as_u64() & 0xFFF; // Conserver les bits PCID
    core::arch::asm!(
        "mov cr3, {}",
        in(reg) pml4.as_u64() | pcid_bits,
        options(nomem, nostack),
    );
}

/// Invalide une seule entrée TLB pour l'adresse virtuelle donnée.
///
/// SAFETY: addr doit être une adresse canonique x86_64.
#[inline]
pub unsafe fn invlpg(addr: VirtAddr) {
    core::arch::asm!(
        "invlpg [{}]",
        in(reg) addr.as_u64(),
        options(nostack, preserves_flags),
    );
}
