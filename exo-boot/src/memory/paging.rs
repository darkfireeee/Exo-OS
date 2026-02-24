//! paging.rs — Configuration des tables de pages initiales du bootloader.
//!
//! RÈGLE ARCH-01 (DOC1) :
//!   "Le bootloader établit un mapping initial :
//!    - Identité : 0–4 GiB (pour le bootloader lui-même et les MMIO)
//!    - Higher-half : FFFF_FFFF_8000_0000 → 0 (pour le kernel — PML4 index 511)"
//!
//! SYNCHRONISATION : KERNEL_HIGHER_HALF_BASE = 0xFFFF_FFFF_8000_0000 (PML4[511]).
//!   Aligné sur kernel/src/arch/x86_64/mod.rs::KERNEL_BASE.
//!
//! Ce module gère :
//!   1. Tables de pages temporaires UEFI (avant ExitBootServices)
//!   2. Tables de pages finales pour le handoff au kernel
//!
//! Architecture : PML4 → PDPT → PD → PT (4 niveaux x86_64)
//! Taille de page : 4 KiB standard (PT entries) ou 2 MiB large pages (PD entries)
//!
//! Les tables sont allouées dans de la mémoire physique contiguë (UEFI AllocatePages
//! ou, en mode BIOS, dans l'espace réservé par stage2.asm à 0x70000).
//!
//! CONTRAT avec handoff.rs : `PageTablesSetup.pml4_phys` est chargé dans CR3
//! juste avant le saut vers le kernel.

use super::{PAGE_SIZE, HUGE_PAGE_SIZE, KERNEL_HIGHER_HALF_BASE};

// ─── Constantes de table de pages ─────────────────────────────────────────────

/// Nombre d'entrées dans une table de pages (tous niveaux).
pub const ENTRIES_PER_TABLE: usize = 512;

/// Taille en octets d'une table de pages.
pub const TABLE_SIZE: usize = ENTRIES_PER_TABLE * 8; // 4096 bytes = PAGE_SIZE

/// Flags de page entry (bits bas de l'entrée 64 bits).
pub mod flags {
    /// Bit 0 : Present — entrée valide.
    pub const PRESENT:     u64 = 1 << 0;
    /// Bit 1 : Read/Write — écriture autorisée.
    pub const WRITABLE:    u64 = 1 << 1;
    /// Bit 2 : User/Supervisor — accessible en mode user (ring 3).
    pub const USER:        u64 = 1 << 2;
    /// Bit 3 : Write-Through — pas de write-back cache.
    pub const WRITE_THRU:  u64 = 1 << 3;
    /// Bit 4 : Cache Disable — pas de cache.
    pub const NO_CACHE:    u64 = 1 << 4;
    /// Bit 5 : Accessed — mis par CPU lors d'un accès.
    pub const ACCESSED:    u64 = 1 << 5;
    /// Bit 6 : Dirty — mis par CPU lors d'une écriture (PT seulement).
    pub const DIRTY:       u64 = 1 << 6;
    /// Bit 7 (PD/PDPT) : Page Size — large page (2 MiB ou 1 GiB).
    pub const HUGE:        u64 = 1 << 7;
    /// Bit 8 : Global — non invalidé par TLB flush (PGE doit être activé).
    pub const GLOBAL:      u64 = 1 << 8;
    /// Bit 63 : Execute Disable (NX) — requiert EFER.NXE = 1.
    pub const NO_EXECUTE:  u64 = 1 << 63;

    /// Entrée standard Read/Write/Present.
    pub const RW_PRESENT:  u64 = PRESENT | WRITABLE;
    /// Large page 2 MiB Read/Write/Present.
    pub const HUGE_RW:     u64 = PRESENT | WRITABLE | HUGE;
}

// ─── Abstraction d'une table de pages ─────────────────────────────────────────

/// Référence mutable vers une table de pages en mémoire physique.
/// Garantit un access aligné à la taille de page.
pub struct PageTable {
    /// Pointeur vers la table (adresse physique identité-mappée).
    ptr: *mut [u64; ENTRIES_PER_TABLE],
}

impl PageTable {
    /// SAFETY : `phys_addr` doit pointer vers 4096 bytes alloués et alignés à PAGE_SIZE.
    unsafe fn from_phys(phys_addr: u64) -> Self {
        debug_assert!(phys_addr % PAGE_SIZE as u64 == 0,
            "Table de pages non alignée : {:#x}", phys_addr);
        Self { ptr: phys_addr as *mut [u64; ENTRIES_PER_TABLE] }
    }

    /// Lit l'entrée à l'index `idx`.
    fn read(&self, idx: usize) -> u64 {
        debug_assert!(idx < ENTRIES_PER_TABLE);
        // SAFETY : pointeur valide et index borné.
        unsafe { core::ptr::read_volatile(&(*self.ptr)[idx]) }
    }

    /// Écrit l'entrée à l'index `idx`.
    fn write(&mut self, idx: usize, val: u64) {
        debug_assert!(idx < ENTRIES_PER_TABLE);
        // SAFETY : pointeur valide et index borné.
        unsafe { core::ptr::write_volatile(&mut (*self.ptr)[idx], val) }
    }

    /// Met à zéro toute la table.
    #[allow(dead_code)]
    fn zero(&mut self) {
        // SAFETY : pointeur valide, taille connue.
        unsafe { core::ptr::write_bytes(self.ptr as *mut u8, 0, TABLE_SIZE) }
    }

    /// Adresse physique de la table.
    #[allow(dead_code)]
    #[inline]
    fn phys_addr(&self) -> u64 { self.ptr as u64 }
}

// ─── Allocateur de tables ─────────────────────────────────────────────────────

/// Allocateur simple pour les tables de pages.
/// Utilise un pool de pages pré-alloué.
struct TableAllocator {
    pool_base: u64,
    pool_size: usize,
    next_page: u64,
}

impl TableAllocator {
    fn new(pool_base: u64, pool_size: usize) -> Self {
        debug_assert!(pool_base % PAGE_SIZE as u64 == 0);
        debug_assert!(pool_size % PAGE_SIZE == 0);
        Self { pool_base, pool_size, next_page: pool_base }
    }

    fn allocate_table(&mut self) -> Result<u64, PageTablesError> {
        let used = (self.next_page - self.pool_base) as usize;
        if used + PAGE_SIZE > self.pool_size {
            return Err(PageTablesError::PoolExhausted {
                used_pages: used / PAGE_SIZE,
                total_pages: self.pool_size / PAGE_SIZE,
            });
        }
        let addr = self.next_page;
        // Zéro-fill la table fraîchement allouée
        unsafe { core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE) };
        self.next_page += PAGE_SIZE as u64;
        Ok(addr)
    }
}

// ─── Structure PageTablesSetup ─────────────────────────────────────────────────

/// Résultat de la construction des tables de pages.
///
/// Transmis à `handoff_to_kernel()` via `BootInfo` / registres.
#[derive(Debug, Clone, Copy)]
pub struct PageTablesSetup {
    /// Adresse physique du PML4 — à charger dans CR3.
    pub pml4_phys:       u64,
    /// Adresse physique du pool de tables (pour BootInfo — reclaimable après init).
    pub pool_phys:       u64,
    /// Taille du pool en octets.
    pub pool_size:       usize,
}

// ─── Construction UEFI ────────────────────────────────────────────────────────

/// Nombre de pages pour le pool de tables de pages (UEFI).
/// 32 tables × 4096 bytes = 128 KiB, suffisant pour identité 4 GiB + higher-half.
const UEFI_PAGE_TABLE_POOL_PAGES: usize = 32;

/// Construit les tables de pages finales pour le handoff (chemin UEFI).
///
/// Mapping :
///   - Identité [0, 4 GiB]   → Large pages 2 MiB (512 entrées dans 4 PDPT entries)
///   - Higher-half [FFFF_FFFF_8000_0000, +4 GiB] → même mapping physique (PML4[511])
///
/// RULES :
///   - BOOT-06 : Cette fonction doit être appelée APRÈS ExitBootServices
///               (elle alloue depuis la carte mémoire, pas via BootServices).
///   - ARCH-01 : Mapping identité 0–4 GiB + higher-half obligatoire.
///
/// `pool_phys` : adresse physique du pool de 128 KiB pré-alloué par caller.
pub fn setup_kernel_page_tables(pool_phys: u64) -> Result<PageTablesSetup, PageTablesError> {
    let pool_size = UEFI_PAGE_TABLE_POOL_PAGES * PAGE_SIZE;
    let mut alloc = TableAllocator::new(pool_phys, pool_size);

    // ── PML4 ──────────────────────────────────────────────────────────────
    let pml4_phys = alloc.allocate_table()?;
    let mut pml4 = unsafe { PageTable::from_phys(pml4_phys) };

    // ── PDPT identité (entrée PML4 index 0 : adresses 0–512 GiB) ──────────
    let pdpt_identity_phys = alloc.allocate_table()?;
    let mut pdpt_identity = unsafe { PageTable::from_phys(pdpt_identity_phys) };

    // Map 4 entrées PDPT avec chacune un PD de 512 × 2 MiB large pages = 4 × 1 GiB = 4 GiB
    for i in 0..4usize {
        let pd_phys = alloc.allocate_table()?;
        let mut pd  = unsafe { PageTable::from_phys(pd_phys) };

        let base_2mib = (i as u64) * 512 * HUGE_PAGE_SIZE as u64;
        for j in 0..ENTRIES_PER_TABLE {
            let phys = base_2mib + (j as u64) * HUGE_PAGE_SIZE as u64;
            pd.write(j, phys | flags::HUGE_RW | flags::GLOBAL);
        }

        pdpt_identity.write(i, pd_phys | flags::RW_PRESENT);
    }

    pml4.write(0, pdpt_identity_phys | flags::RW_PRESENT);

    // ── PDPT higher-half (entrée PML4 index 511 : FFFF_FFFF_8000_0000) ────
    // Index PML4 pour KERNEL_HIGHER_HALF_BASE = bits 47:39 = 511
    const HIGHER_HALF_PML4_IDX: usize = ((KERNEL_HIGHER_HALF_BASE >> 39) & 0x1FF) as usize;
    let pdpt_higher_phys = alloc.allocate_table()?;
    let mut pdpt_higher  = unsafe { PageTable::from_phys(pdpt_higher_phys) };

    // Réutilise les mêmes PD que pour l'identité (même mapping physique)
    for i in 0..4usize {
        // Lit l'entrée du PDPT identité pour récupérer l'adresse du PD
        let pd_entry = pdpt_identity.read(i);
        pdpt_higher.write(i, pd_entry); // Même PD, same flags
    }

    // HIGHER_HALF_PML4_IDX = 511 (calculé depuis 0xFFFF_FFFF_8000_0000)
    // NOTE : Pas de self-map ici — le kernel établit ses propres tables de pages
    //        après l'init mémoire (crate::memory::virt::address_space::KERNEL_AS).
    pml4.write(HIGHER_HALF_PML4_IDX, pdpt_higher_phys | flags::RW_PRESENT);

    Ok(PageTablesSetup {
        pml4_phys,
        pool_phys,
        pool_size,
    })
}

/// Alloue le pool de tables de pages via UEFI AllocatePages.
///
/// RÈGLE BOOT-06 : Doit être appelé AVANT ExitBootServices.
/// Le pool sera ensuite utilisé par `setup_kernel_page_tables()`.
#[cfg(feature = "uefi-boot")]
pub fn allocate_page_table_pool(
    bt: &uefi::table::boot::BootServices,
) -> Result<u64, PageTablesError> {
    use uefi::table::boot::{AllocateType, MemoryType};

    let pages = UEFI_PAGE_TABLE_POOL_PAGES;
    let addr  = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
        .map_err(|_| PageTablesError::AllocationFailed)?;

    // Zéro-fill pour sécurité
    unsafe { core::ptr::write_bytes(addr as *mut u8, 0, pages * PAGE_SIZE) };

    Ok(addr)
}

// ─── Construction BIOS ────────────────────────────────────────────────────────

/// Adresse fixe du pool BIOS (stage2.asm réserve 0x70000–0x78000 = 32 KiB).
pub const BIOS_PAGE_TABLE_POOL: u64   = 0x0007_0000;
/// Taille du pool BIOS.
pub const BIOS_PAGE_TABLE_POOL_SIZE: usize = 8 * PAGE_SIZE; // 32 KiB

/// Construit les tables de pages dans le pool BIOS reservé par stage2.asm.
///
/// RÈGLE : Appelé depuis exoboot_main_bios() avant le handoff.
/// Note : stage2.asm a déjà initialisé ses propres tables identité temporaires
/// (à 0x70000) pour passer en long mode. Cette fonction les REMPLACE par les
/// tables finales du kernel avec mapping higher-half.
pub fn setup_kernel_page_tables_bios() -> Result<PageTablesSetup, PageTablesError> {
    // Zéro-fill le pool BIOS avant utilisation
    unsafe {
        core::ptr::write_bytes(
            BIOS_PAGE_TABLE_POOL as *mut u8,
            0,
            BIOS_PAGE_TABLE_POOL_SIZE,
        )
    };
    setup_kernel_page_tables(BIOS_PAGE_TABLE_POOL)
}

// ─── Invalidation TLB ─────────────────────────────────────────────────────────

/// Invalide TLB entier via reload CR3.
/// Appelé après modification des tables de pages.
#[inline]
pub fn flush_tlb_full(new_cr3: u64) {
    // SAFETY : Écriture dans CR3 — invalide tous les TLB entries.
    unsafe {
        core::arch::asm!(
            "mov cr3, {0}",
            in(reg) new_cr3,
            options(nostack, preserves_flags),
        );
    }
}

/// Active la pagination NX (NXE) dans EFER.
/// Doit être fait avant de charger des tables avec bit NX.
#[inline]
pub fn enable_nxe() {
    const IA32_EFER: u32 = 0xC000_0080;
    const EFER_NXE:  u64 = 1 << 11;

    // SAFETY : Accès MSR en mode ring 0 (bootloader).
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_EFER,
            out("eax") lo,
            out("edx") hi,
            options(nostack),
        );
        let mut efer: u64 = ((hi as u64) << 32) | (lo as u64);
        efer |= EFER_NXE;
        let lo = efer as u32;
        let hi = (efer >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_EFER,
            in("eax") lo,
            in("edx") hi,
            options(nostack),
        );
    }
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PageTablesError {
    /// Pool de tables de pages épuisé.
    PoolExhausted { used_pages: usize, total_pages: usize },
    /// Allocation UEFI échouée.
    AllocationFailed,
    /// Adresse non-alignée fournie.
    Misaligned { addr: u64, alignment: usize },
}

impl core::fmt::Display for PageTablesError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PoolExhausted { used_pages, total_pages } =>
                write!(f, "Pool tables de pages épuisé : {}/{} pages", used_pages, total_pages),
            Self::AllocationFailed =>
                write!(f, "Allocation UEFI échouée pour tables de pages"),
            Self::Misaligned { addr, alignment } =>
                write!(f, "Adresse {:#x} non alignée sur {} bytes", addr, alignment),
        }
    }
}
