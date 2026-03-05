// kernel/src/memory/core/address.rs
//
// Translations d'adresses, assertions d'alignement, utilitaires.
// Couche 0 — aucune dépendance externe.

use super::types::{PhysAddr, VirtAddr, Frame, Page};
use super::constants::{PAGE_SIZE, PAGE_SHIFT, HUGE_PAGE_SIZE, HUGE_PAGE_SHIFT};
use super::layout::{
    KERNEL_PHYS_OFFSET, PHYS_MAP_BASE, PHYS_MAP_SIZE,
    KERNEL_HEAP_START, KERNEL_HEAP_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// TRANSLATIONS PHYSIQUE ↔ VIRTUEL (via la région physmap du noyau)
// ─────────────────────────────────────────────────────────────────────────────

/// Traduit une adresse physique en adresse virtuelle via la physmap directe.
///
/// La physmap mappe l'intégralité de la RAM physique à PHYS_MAP_BASE en
/// lecture/écriture (mode kernel uniquement). Toujours valide pendant
/// l'exécution du noyau après init_phys_map().
///
/// # Panics
/// En mode debug, panique si l'adresse dépasse PHYS_MAP_SIZE.
#[inline(always)]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    debug_assert!(
        phys.as_u64() < PHYS_MAP_SIZE as u64,
        "phys_to_virt: adresse physique 0x{:016x} dépasse la physmap (max 0x{:016x})",
        phys.as_u64(),
        PHYS_MAP_SIZE
    );
    // SAFETY: physmap initialisée au boot; offset dans l'espace canonique kernel.
    unsafe { VirtAddr::new_unchecked(PHYS_MAP_BASE.as_u64() + phys.as_u64()) }
}

/// Traduit une adresse virtuelle physmap en adresse physique.
///
/// # Panics
/// En mode debug, panique si l'adresse n'est pas dans la physmap.
#[inline(always)]
pub fn virt_to_phys_physmap(virt: VirtAddr) -> PhysAddr {
    debug_assert!(
        virt >= PHYS_MAP_BASE && virt.as_u64() < PHYS_MAP_BASE.as_u64() + PHYS_MAP_SIZE as u64,
        "virt_to_phys_physmap: adresse 0x{:016x} hors physmap",
        virt.as_u64()
    );
    PhysAddr::new(virt.as_u64() - PHYS_MAP_BASE.as_u64())
}

/// Traduit une adresse virtuelle kernel (section .text/.data/.bss) en physique.
/// Applicable uniquement pour les adresses dans le mapping kernel direct.
///
/// # Panics
/// En mode debug, panique si l'adresse précède KERNEL_PHYS_OFFSET.
#[inline(always)]
pub fn kernel_virt_to_phys(virt: VirtAddr) -> PhysAddr {
    debug_assert!(
        virt.as_u64() >= KERNEL_PHYS_OFFSET.as_u64(),
        "kernel_virt_to_phys: adresse 0x{:016x} avant KERNEL_PHYS_OFFSET",
        virt.as_u64()
    );
    PhysAddr::new(virt.as_u64() - KERNEL_PHYS_OFFSET.as_u64())
}

/// Traduit une adresse physique kernel en adresse virtuelle .text/.data.
#[inline(always)]
pub fn kernel_phys_to_virt(phys: PhysAddr) -> VirtAddr {
    // SAFETY: Arithmétique sur l'espace d'adressage canonique.
    unsafe { VirtAddr::new_unchecked(phys.as_u64() + KERNEL_PHYS_OFFSET.as_u64()) }
}

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS D'ALIGNEMENT
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que `size` est non nul et puissance de 2.
#[inline(always)]
pub const fn is_power_of_two(size: usize) -> bool {
    size != 0 && (size & (size - 1)) == 0
}

/// Arrondit `value` vers le bas au multiple de `align`.
/// `align` doit être une puissance de 2.
#[inline(always)]
pub const fn align_down(value: usize, align: usize) -> usize {
    debug_assert!(is_power_of_two(align));
    value & !(align - 1)
}

/// Arrondit `value` vers le haut au multiple de `align`.
/// `align` doit être une puissance de 2.
#[inline(always)]
pub const fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(is_power_of_two(align));
    (value + align - 1) & !(align - 1)
}

/// Vérifie si `value` est aligné sur `align`.
#[inline(always)]
pub const fn is_aligned(value: usize, align: usize) -> bool {
    (value & (align - 1)) == 0
}

/// Arrondit vers le bas à la page.
#[inline(always)]
pub const fn page_align_down(value: usize) -> usize {
    align_down(value, PAGE_SIZE)
}

/// Arrondit vers le haut à la page.
#[inline(always)]
pub const fn page_align_up(value: usize) -> usize {
    align_up(value, PAGE_SIZE)
}

/// Nombre de pages nécessaires pour couvrir `size` octets.
#[inline(always)]
pub const fn pages_for(size: usize) -> usize {
    (size + PAGE_SIZE - 1) >> PAGE_SHIFT
}

/// Nombre de huge pages (2 MiB) nécessaires pour couvrir `size` octets.
#[inline(always)]
pub const fn huge_pages_for(size: usize) -> usize {
    (size + HUGE_PAGE_SIZE - 1) >> HUGE_PAGE_SHIFT
}

/// Octets → pages (arrondi au-dessus).
#[inline(always)]
pub const fn bytes_to_pages(bytes: usize) -> usize {
    pages_for(bytes)
}

/// Pages → octets.
#[inline(always)]
pub const fn pages_to_bytes(pages: usize) -> usize {
    pages << PAGE_SHIFT
}

// ─────────────────────────────────────────────────────────────────────────────
// CALCULS SUR FRAMES / PAGES
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le Frame contenant l'adresse physique `addr`.
#[inline(always)]
pub const fn frame_containing(addr: PhysAddr) -> Frame {
    Frame::containing(addr)
}

/// Retourne la Page contenant l'adresse virtuelle `addr`.
#[inline(always)]
pub const fn page_containing(addr: VirtAddr) -> Page {
    Page::containing(addr)
}

/// Retourne l'adresse physique d'un PFN.
#[inline(always)]
pub const fn pfn_to_phys(pfn: u64) -> PhysAddr {
    PhysAddr::new(pfn << PAGE_SHIFT as u64)
}

/// Retourne le PFN d'une adresse physique (arrondi au-dessous).
#[inline(always)]
pub const fn phys_to_pfn(addr: PhysAddr) -> u64 {
    addr.as_u64() >> PAGE_SHIFT as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// CANONICALISATION x86_64
// ─────────────────────────────────────────────────────────────────────────────

/// Canonicalise une adresse virtuelle 64 bits (sign-extension bit 47).
/// Requis avant tout chargement dans CR3 ou accès à la table des pages.
#[inline(always)]
pub const fn canonicalize(addr: u64) -> u64 {
    // Shift gauche 16, puis arithmetic shift droit 16 = sign-extend bit 47
    (((addr << 16) as i64) >> 16) as u64
}

/// Vérifie si une adresse est canonique selon les règles x86_64.
/// Les adresses non canoniques déclenchent un #GP fault.
#[inline(always)]
pub const fn is_canonical(addr: u64) -> bool {
    let extended = canonicalize(addr);
    extended == addr
}

/// Vérifie si un pointeur est utilisable comme adresse noyau canonique.
#[inline(always)]
pub const fn is_kernel_canonical(addr: u64) -> bool {
    is_canonical(addr) && addr >= 0xFFFF_8000_0000_0000
}

/// Vérifie si un pointeur est utilisable comme adresse utilisateur canonique.
#[inline(always)]
pub const fn is_user_canonical(addr: u64) -> bool {
    is_canonical(addr) && addr < 0x0000_8000_0000_0000
}

// ─────────────────────────────────────────────────────────────────────────────
// PLAGES D'ADRESSES
// ─────────────────────────────────────────────────────────────────────────────

/// Plage d'adresses physiques [start, end).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PhysRange {
    pub start: PhysAddr,
    pub end:   PhysAddr,
}

impl PhysRange {
    /// Crée une plage [start, start + size).
    #[inline(always)]
    pub const fn new(start: PhysAddr, size: usize) -> Self {
        PhysRange { start, end: PhysAddr::new(start.as_u64() + size as u64) }
    }

    /// Crée une plage [start, end).
    #[inline(always)]
    pub const fn from_range(start: PhysAddr, end: PhysAddr) -> Self {
        PhysRange { start, end }
    }

    /// Taille en octets.
    #[inline(always)]
    pub const fn size(self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Nombre de pages dans la plage (arrondi au-dessus).
    #[inline(always)]
    pub fn page_count(self) -> usize {
        pages_for(self.size())
    }

    /// Vérifie si la plage contient `addr`.
    #[inline(always)]
    pub const fn contains(self, addr: PhysAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Vérifie si la plage est vide.
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.start.as_u64() >= self.end.as_u64()
    }

    /// Vérifie si deux plages se chevauchent.
    #[inline(always)]
    pub const fn overlaps(self, other: PhysRange) -> bool {
        self.start.as_u64() < other.end.as_u64()
            && other.start.as_u64() < self.end.as_u64()
    }

    /// Alignement de start vers le bas sur une page.
    #[inline(always)]
    pub fn page_aligned(self) -> PhysRange {
        PhysRange {
            start: self.start.page_align_down(),
            end:   self.end.page_align_up(),
        }
    }
}

/// Plage d'adresses virtuelles [start, end).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct VirtRange {
    pub start: VirtAddr,
    pub end:   VirtAddr,
}

impl VirtRange {
    /// Crée une plage [start, start + size).
    #[inline(always)]
    pub const fn new(start: VirtAddr, size: usize) -> Self {
        VirtRange {
            start,
            end: unsafe { VirtAddr::new_unchecked(start.as_u64() + size as u64) },
        }
    }

    /// Crée une plage [start, end).
    #[inline(always)]
    pub const fn from_range(start: VirtAddr, end: VirtAddr) -> Self {
        VirtRange { start, end }
    }

    /// Taille en octets.
    #[inline(always)]
    pub const fn size(self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Nombre de pages dans la plage.
    #[inline(always)]
    pub fn page_count(self) -> usize {
        pages_for(self.size())
    }

    /// Vérifie si la plage contient `addr`.
    #[inline(always)]
    pub const fn contains(self, addr: VirtAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Vérifie si les deux plages se chevauchent.
    #[inline(always)]
    pub const fn overlaps(self, other: VirtRange) -> bool {
        self.start.as_u64() < other.end.as_u64()
            && other.start.as_u64() < self.end.as_u64()
    }

    /// Aligne les bornes sur des pages.
    #[inline(always)]
    pub fn page_aligned(self) -> VirtRange {
        VirtRange {
            start: self.start.page_align_down(),
            end:   self.end.page_align_up(),
        }
    }

    /// Vérifie si la plage est entièrement dans l'espace noyau.
    #[inline(always)]
    pub const fn is_kernel(self) -> bool {
        self.start.is_kernel() && self.end.is_kernel()
    }

    /// Vérifie si la plage est entièrement dans l'espace utilisateur.
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.start.is_user() && self.end.is_user()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS HEAP NOYAU
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si une adresse virtuelle appartient au heap kernel.
#[inline(always)]
pub fn in_kernel_heap(addr: VirtAddr) -> bool {
    addr >= KERNEL_HEAP_START
        && addr.as_u64() < KERNEL_HEAP_START.as_u64() + KERNEL_HEAP_SIZE as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// TESTS INLINE (vérifiables en mode debug)
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute les assertions de cohérence du module address au boot.
pub fn assert_invariants() {
    // Cohérence PAGE_SIZE / PAGE_SHIFT
    debug_assert_eq!(1usize << PAGE_SHIFT, PAGE_SIZE);
    // Cohérence HUGE_PAGE_SIZE / HUGE_PAGE_SHIFT
    debug_assert_eq!(1usize << HUGE_PAGE_SHIFT, HUGE_PAGE_SIZE);
    // La physmap commence dans l'espace noyau
    debug_assert!(PHYS_MAP_BASE.is_kernel());
    // align_down(0, PAGE_SIZE) == 0
    debug_assert_eq!(page_align_down(0), 0);
    // align_up(1, PAGE_SIZE) == PAGE_SIZE
    debug_assert_eq!(page_align_up(1), PAGE_SIZE);
    // align_up(PAGE_SIZE, PAGE_SIZE) == PAGE_SIZE
    debug_assert_eq!(page_align_up(PAGE_SIZE), PAGE_SIZE);
    // pages_for(0) == 0
    debug_assert_eq!(pages_for(0), 0);
    // pages_for(1) == 1
    debug_assert_eq!(pages_for(1), 1);
    // pages_for(PAGE_SIZE) == 1
    debug_assert_eq!(pages_for(PAGE_SIZE), 1);
    // pages_for(PAGE_SIZE + 1) == 2
    debug_assert_eq!(pages_for(PAGE_SIZE + 1), 2);
    // canonicalize préserve les adresses canoniques
    debug_assert_eq!(canonicalize(0xFFFF_8000_0000_0000), 0xFFFF_8000_0000_0000);
    debug_assert_eq!(canonicalize(0x0000_7FFF_FFFF_FFFF), 0x0000_7FFF_FFFF_FFFF);
    // Une adresse non-canonique doit être détectée
    debug_assert!(!is_canonical(0x0001_0000_0000_0000));
}

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS STATIQUES
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(is_power_of_two(PAGE_SIZE),       "PAGE_SIZE doit être puissance de 2");
const _: () = assert!(is_power_of_two(HUGE_PAGE_SIZE),  "HUGE_PAGE_SIZE doit être puissance de 2");
const _: () = assert!(PAGE_SIZE == 4096,                "PAGE_SIZE doit être 4096 pour x86_64");
const _: () = assert!(HUGE_PAGE_SIZE == 2097152,        "HUGE_PAGE_SIZE doit être 2MiB");
const _: () = assert!(align_up(0, PAGE_SIZE)  == 0,     "align_up(0, PAGE_SIZE) doit être 0");
const _: () = assert!(align_up(1, PAGE_SIZE)  == PAGE_SIZE, "align_up(1, PAGE_SIZE) doit être PAGE_SIZE");
const _: () = assert!(align_down(PAGE_SIZE + 1, PAGE_SIZE) == PAGE_SIZE, "align_down doit arrondir vers le bas");
