// kernel/src/memory/core/types.rs
//
// Types fondamentaux du module memory — Exo-OS Couche 0
// Aucune dépendance externe au kernel.
// Tous les types sont testés statiquement pour leur taille/alignement.

use super::constants::{HUGE_PAGE_SIZE, PAGE_MASK, PAGE_SHIFT};
use core::fmt;
use core::ops::{Add, AddAssign, BitAnd, BitOr, Not, Sub, SubAssign};

// ─────────────────────────────────────────────────────────────────────────────
// PHYSADDR — Adresse physique
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse physique 64 bits — opaque, ne peut pas être déréférenciée directement.
/// Toujours canonique (bits 48..63 = 0 sur x86_64 sans 5-level paging).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Crée une PhysAddr depuis une valeur brute.
    /// Panics (debug) si non canonique.
    #[inline(always)]
    pub const fn new(addr: u64) -> Self {
        // En no_std, on ne peut pas paniquer en const — on accepte silencieusement
        // La validation se fait à l'utilisation via is_canonical()
        PhysAddr(addr)
    }

    /// Crée une PhysAddr sans aucune vérification (unsafe).
    /// SAFETY: L'appelant garantit que l'adresse est physiquement valide
    /// et dans la plage adressable du matériel.
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        PhysAddr(addr)
    }

    /// Retourne la valeur brute de l'adresse.
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Retourne la valeur brute sous forme usize (truncature possible sur 32 bits).
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Vérifie si l'adresse est alignée sur la taille indiquée (doit être puissance de 2).
    #[inline(always)]
    pub const fn is_aligned(self, align: u64) -> bool {
        debug_assert!(align.is_power_of_two());
        (self.0 & (align - 1)) == 0
    }

    /// Aligne l'adresse vers le bas sur `align` octets.
    #[inline(always)]
    pub const fn align_down(self, align: u64) -> Self {
        PhysAddr(self.0 & !(align - 1))
    }

    /// Aligne l'adresse vers le haut sur `align` octets.
    #[inline(always)]
    pub const fn align_up(self, align: u64) -> Self {
        PhysAddr((self.0 + align - 1) & !(align - 1))
    }

    /// Retourne l'index du frame physique (PFN = Physical Frame Number).
    #[inline(always)]
    pub const fn pfn(self) -> u64 {
        self.0 >> PAGE_SHIFT as u64
    }

    /// Aligne sur une page vers le bas.
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        PhysAddr(self.0 & !(PAGE_MASK as u64))
    }

    /// Aligne sur une page vers le haut.
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        PhysAddr((self.0 + PAGE_MASK as u64) & !(PAGE_MASK as u64))
    }

    /// Retourne `true` si alignée sur une page.
    #[inline(always)]
    pub const fn is_page_aligned(self) -> bool {
        (self.0 & PAGE_MASK as u64) == 0
    }

    /// Additionne un offset en octets.
    #[inline(always)]
    pub const fn add(self, offset: u64) -> Self {
        PhysAddr(self.0.wrapping_add(offset))
    }

    /// Soustrait un offset en octets.
    #[inline(always)]
    pub const fn sub(self, offset: u64) -> Self {
        PhysAddr(self.0.wrapping_sub(offset))
    }

    /// Différence entre deux adresses physiques (self - other).
    /// Panics (debug) si self < other.
    #[inline(always)]
    pub fn offset_from(self, base: PhysAddr) -> u64 {
        debug_assert!(self >= base, "PhysAddr::offset_from: self < base");
        self.0 - base.0
    }

    /// Adresse nulle (non valide, utilisée comme sentinelle).
    pub const NULL: PhysAddr = PhysAddr(0);

    /// Valeur maximale representable.
    pub const MAX: PhysAddr = PhysAddr(u64::MAX);
}

impl Add<u64> for PhysAddr {
    type Output = PhysAddr;
    #[inline(always)]
    fn add(self, rhs: u64) -> PhysAddr {
        PhysAddr(self.0.wrapping_add(rhs))
    }
}

impl AddAssign<u64> for PhysAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl Sub<u64> for PhysAddr {
    type Output = PhysAddr;
    #[inline(always)]
    fn sub(self, rhs: u64) -> PhysAddr {
        PhysAddr(self.0.wrapping_sub(rhs))
    }
}

impl Sub<PhysAddr> for PhysAddr {
    type Output = u64;
    #[inline(always)]
    fn sub(self, rhs: PhysAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for PhysAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysAddr(0x{:016x})", self.0)
    }
}

impl fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

impl fmt::LowerHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VIRTADDR — Adresse virtuelle
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse virtuelle canonique 64 bits (x86_64: bits 48..63 = sign-extend bit 47).
/// Ne peut PAS être convertie en pointeur sans vérification de mapping.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Crée une VirtAddr depuis une valeur brute.
    /// Canonicalise automatiquement (sign-extend bit 47).
    #[inline(always)]
    pub const fn new(addr: u64) -> Self {
        // Sign-extend bit 47 vers bits 48..63
        let bits = addr << 16;
        VirtAddr((bits as i64 >> 16) as u64)
    }

    /// Crée sans vérification.
    /// SAFETY: L'appelant garantit que addr est une adresse canonique valide.
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        VirtAddr(addr)
    }

    /// Crée depuis un pointeur brut.
    /// SAFETY: Le pointeur doit être une adresse virtuelle canonique valide.
    #[inline(always)]
    pub unsafe fn from_ptr<T>(ptr: *const T) -> Self {
        VirtAddr(ptr as u64)
    }

    /// Retourne la valeur brute.
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Retourne la valeur brute sous forme usize.
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Convertit en pointeur brut *const T.
    /// SAFETY: L'appelant doit s'assurer que la page est mappée,
    /// alignée correctement, et que la durée de vie est valide.
    #[inline(always)]
    pub const unsafe fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Convertit en pointeur brut *mut T.
    /// SAFETY: Comme as_ptr, plus : la page doit être writable.
    #[inline(always)]
    pub const unsafe fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Vérifie si l'adresse est canonique (x86_64: bits 48..63 = sign-extend 47).
    #[inline(always)]
    pub const fn is_canonical(self) -> bool {
        let high = self.0 >> 48;
        high == 0 || high == 0xFFFF
    }

    /// Vérifie si l'adresse est dans l'espace utilisateur (< 0x0000_8000_0000_0000).
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.0 < 0x0000_8000_0000_0000
    }

    /// Vérifie si l'adresse est dans l'espace noyau (>= 0xFFFF_8000_0000_0000).
    #[inline(always)]
    pub const fn is_kernel(self) -> bool {
        self.0 >= 0xFFFF_8000_0000_0000
    }

    /// Aligne vers le bas sur `align` octets (puissance de 2).
    #[inline(always)]
    pub const fn align_down(self, align: u64) -> Self {
        VirtAddr::new(self.0 & !(align - 1))
    }

    /// Aligne vers le haut sur `align` octets (puissance de 2).
    #[inline(always)]
    pub const fn align_up(self, align: u64) -> Self {
        VirtAddr::new((self.0 + align - 1) & !(align - 1))
    }

    /// Aligne sur une page vers le bas.
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        VirtAddr::new(self.0 & !(PAGE_MASK as u64))
    }

    /// Aligne sur une page vers le haut.
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        VirtAddr::new((self.0 + PAGE_MASK as u64) & !(PAGE_MASK as u64))
    }

    /// Index de page (numéro de page virtuelle).
    #[inline(always)]
    pub const fn vpn(self) -> u64 {
        self.0 >> PAGE_SHIFT as u64
    }

    /// Extrait l'index PML4 (bits 39..48).
    #[inline(always)]
    pub const fn p4_index(self) -> usize {
        ((self.0 >> 39) & 0x1FF) as usize
    }

    /// Extrait l'index PDPT (bits 30..39).
    #[inline(always)]
    pub const fn p3_index(self) -> usize {
        ((self.0 >> 30) & 0x1FF) as usize
    }

    /// Extrait l'index PD (bits 21..30).
    #[inline(always)]
    pub const fn p2_index(self) -> usize {
        ((self.0 >> 21) & 0x1FF) as usize
    }

    /// Extrait l'index PT (bits 12..21).
    #[inline(always)]
    pub const fn p1_index(self) -> usize {
        ((self.0 >> 12) & 0x1FF) as usize
    }

    /// Extrait l'offset dans la page (bits 0..12).
    #[inline(always)]
    pub const fn page_offset(self) -> usize {
        (self.0 & PAGE_MASK as u64) as usize
    }

    /// Extrait l'offset dans la huge page (bits 0..21).
    #[inline(always)]
    pub const fn huge_page_offset(self) -> usize {
        (self.0 & (HUGE_PAGE_SIZE - 1) as u64) as usize
    }

    /// Différence entre self et base (self - base).
    #[inline(always)]
    pub fn offset_from(self, base: VirtAddr) -> u64 {
        self.0.wrapping_sub(base.0)
    }

    pub const NULL: VirtAddr = VirtAddr(0);
    pub const KERNEL_BASE: VirtAddr = VirtAddr(0xFFFF_8000_0000_0000);
}

impl Add<u64> for VirtAddr {
    type Output = VirtAddr;
    #[inline(always)]
    fn add(self, rhs: u64) -> VirtAddr {
        VirtAddr::new(self.0.wrapping_add(rhs))
    }
}

impl AddAssign<u64> for VirtAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

impl Sub<u64> for VirtAddr {
    type Output = VirtAddr;
    #[inline(always)]
    fn sub(self, rhs: u64) -> VirtAddr {
        VirtAddr::new(self.0.wrapping_sub(rhs))
    }
}

impl Sub<VirtAddr> for VirtAddr {
    type Output = u64;
    #[inline(always)]
    fn sub(self, rhs: VirtAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for VirtAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VirtAddr(0x{:016x})", self.0)
    }
}

impl fmt::Display for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE — Page virtuelle
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'une page virtuelle (VPN — Virtual Page Number).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Page(u64);

impl Page {
    /// Crée une Page depuis une VirtAddr (arrondit vers le bas à la page).
    #[inline(always)]
    pub const fn containing(addr: VirtAddr) -> Self {
        Page(addr.as_u64() >> PAGE_SHIFT as u64)
    }

    /// Retourne le VPN brut.
    #[inline(always)]
    pub const fn vpn(self) -> u64 {
        self.0
    }

    /// Retourne l'adresse virtuelle de début de cette page.
    #[inline(always)]
    pub const fn start_address(self) -> VirtAddr {
        VirtAddr::new(self.0 << PAGE_SHIFT as u64)
    }

    /// Retourne la Page suivante.
    #[inline(always)]
    pub const fn next(self) -> Self {
        Page(self.0 + 1)
    }

    /// Retourne un itérateur sur la plage [self, end).
    #[inline(always)]
    pub fn range(self, end: Page) -> PageRange {
        PageRange { current: self, end }
    }

    /// Nombre de pages séparant self de `end`.
    #[inline(always)]
    pub fn pages_until(self, end: Page) -> u64 {
        end.0 - self.0
    }
}

/// Itérateur sur une plage de pages virtuelles.
pub struct PageRange {
    current: Page,
    end: Page,
}

impl Iterator for PageRange {
    type Item = Page;
    #[inline(always)]
    fn next(&mut self) -> Option<Page> {
        if self.current < self.end {
            let p = self.current;
            self.current = self.current.next();
            Some(p)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = (self.end.0 - self.current.0) as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for PageRange {}

// ─────────────────────────────────────────────────────────────────────────────
// FRAME — Frame physique
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un frame physique (PFN — Physical Frame Number).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Frame(u64);

impl Frame {
    /// Crée un Frame depuis une PhysAddr (arrondit vers le bas).
    #[inline(always)]
    pub const fn containing(addr: PhysAddr) -> Self {
        Frame(addr.as_u64() >> PAGE_SHIFT as u64)
    }

    /// Retourne le PFN brut.
    #[inline(always)]
    pub const fn pfn(self) -> u64 {
        self.0
    }

    /// Retourne l'adresse physique de début de ce frame.
    #[inline(always)]
    pub const fn start_address(self) -> PhysAddr {
        PhysAddr::new(self.0 << PAGE_SHIFT as u64)
    }

    /// Retourne le Frame suivant.
    #[inline(always)]
    pub const fn next(self) -> Self {
        Frame(self.0 + 1)
    }

    /// Itérateur sur la plage [self, end).
    #[inline(always)]
    pub fn range(self, end: Frame) -> FrameRange {
        FrameRange { current: self, end }
    }

    /// Nombre de frames jusqu'à `end`.
    #[inline(always)]
    pub fn frames_until(self, end: Frame) -> u64 {
        end.0 - self.0
    }

    /// Retourne l'adresse physique de début de ce frame.
    /// Alias de `start_address()` — fourni pour cohérence d'API.
    #[inline(always)]
    pub const fn phys_addr(self) -> PhysAddr {
        self.start_address()
    }

    /// Crée un Frame depuis une PhysAddr (arrondit vers le bas vers la page).
    /// Alias de `Frame::containing()` — fourni pour cohérence d'API.
    #[inline(always)]
    pub const fn from_phys_addr(addr: PhysAddr) -> Self {
        Frame::containing(addr)
    }

    /// Frame correspondant au PFN 0 (frame nul, jamais alloué).
    pub const NULL: Frame = Frame(0);
}

/// Itérateur sur une plage de frames physiques.
pub struct FrameRange {
    current: Frame,
    end: Frame,
}

impl Iterator for FrameRange {
    type Item = Frame;
    #[inline(always)]
    fn next(&mut self) -> Option<Frame> {
        if self.current < self.end {
            let f = self.current;
            self.current = self.current.next();
            Some(f)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = (self.end.0 - self.current.0) as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for FrameRange {}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE FLAGS — Drapeaux de protection/attributs de page
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux de page — représentent les bits de l'entrée page table x86_64.
/// Les valeurs correspondent exactement aux bits hardware (P, RW, US, PWT, PCD,
/// A, D, PS, G, NX...) pour un mapping 1:1 avec le matériel.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PageFlags(u64);

impl PageFlags {
    // ── Bits matériels x86_64 ──────────────────────────────────────────────

    /// Bit P : page présente en mémoire physique.
    pub const PRESENT: PageFlags = PageFlags(1 << 0);
    /// Bit RW : lecture/écriture (sinon lecture seule).
    pub const WRITABLE: PageFlags = PageFlags(1 << 1);
    /// Bit US : accessible depuis userspace (ring 3).
    pub const USER: PageFlags = PageFlags(1 << 2);
    /// Bit PWT : write-through (sinon write-back).
    pub const WRITE_THROUGH: PageFlags = PageFlags(1 << 3);
    /// Bit PCD : désactiver le cache (No-Cache).
    pub const NO_CACHE: PageFlags = PageFlags(1 << 4);
    /// Bit A : page accédée (mise à jour par le CPU).
    pub const ACCESSED: PageFlags = PageFlags(1 << 5);
    /// Bit D : page modifiée/sale (Dirty).
    pub const DIRTY: PageFlags = PageFlags(1 << 6);
    /// Bit PS : huge page (2MiB au niveau PD, 1GiB au niveau PDPT).
    pub const HUGE_PAGE: PageFlags = PageFlags(1 << 7);
    /// Bit G : page globale (non invalidée sur changement CR3, TLB global).
    pub const GLOBAL: PageFlags = PageFlags(1 << 8);
    /// Bit NX (bit 63) : No-Execute — interdit l'exécution de code.
    pub const NO_EXECUTE: PageFlags = PageFlags(1 << 63);

    // ── Bits logiciels (bits 9..11, bits 52..62 disponibles pour l'OS) ─────

    /// Bit logiciel 9 : page Copy-on-Write.
    pub const COW: PageFlags = PageFlags(1 << 9);
    /// Bit logiciel 10 : page verrouillée en RAM (ne pas swapper).
    pub const PINNED: PageFlags = PageFlags(1 << 10);
    /// Bit logiciel 11 : shared memory (partagé entre processus).
    pub const SHARED: PageFlags = PageFlags(1 << 11);
    /// Bit logiciel 52 : page utilisée pour DMA (pas de CoW, pas de swap).
    pub const DMA: PageFlags = PageFlags(1 << 52);
    /// Bit logiciel 53 : frame DMA — pas de Write-Combining.
    pub const DMA_NO_WC: PageFlags = PageFlags(1 << 53);

    // ── Combinaisons courantes ─────────────────────────────────────────────

    /// Page noyau exécutable : P + RW + G + NX désactivé.
    pub const KERNEL_CODE: PageFlags = PageFlags(Self::PRESENT.0 | Self::GLOBAL.0);

    /// Page noyau données R/W, non exécutable.
    pub const KERNEL_DATA: PageFlags =
        PageFlags(Self::PRESENT.0 | Self::WRITABLE.0 | Self::GLOBAL.0 | Self::NO_EXECUTE.0);

    /// Page utilisateur R/W, non exécutable.
    pub const USER_DATA: PageFlags =
        PageFlags(Self::PRESENT.0 | Self::WRITABLE.0 | Self::USER.0 | Self::NO_EXECUTE.0);

    /// Page utilisateur exécutable (code).
    pub const USER_CODE: PageFlags = PageFlags(Self::PRESENT.0 | Self::USER.0);

    /// Page DMA coherent (device memory, no-cache, R/W, noyau global).
    pub const KERNEL_DMA: PageFlags = PageFlags(
        Self::PRESENT.0
            | Self::WRITABLE.0
            | Self::GLOBAL.0
            | Self::NO_CACHE.0
            | Self::NO_EXECUTE.0
            | Self::DMA.0,
    );

    /// Page vide (non présente).
    pub const EMPTY: PageFlags = PageFlags(0);

    // ── Constructeur ──────────────────────────────────────────────────────

    #[inline(always)]
    pub const fn new(bits: u64) -> Self {
        PageFlags(bits)
    }

    #[inline(always)]
    pub const fn bits(self) -> u64 {
        self.0
    }

    // ── Opérations booléennes ─────────────────────────────────────────────

    #[inline(always)]
    pub const fn contains(self, other: PageFlags) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline(always)]
    pub const fn intersects(self, other: PageFlags) -> bool {
        (self.0 & other.0) != 0
    }

    #[inline(always)]
    pub const fn set(self, flag: PageFlags) -> Self {
        PageFlags(self.0 | flag.0)
    }

    #[inline(always)]
    pub const fn clear(self, flag: PageFlags) -> Self {
        PageFlags(self.0 & !flag.0)
    }

    #[inline(always)]
    pub const fn toggle(self, flag: PageFlags) -> Self {
        PageFlags(self.0 ^ flag.0)
    }

    /// Vérifie si la page est présente.
    #[inline(always)]
    pub const fn is_present(self) -> bool {
        self.contains(Self::PRESENT)
    }

    /// Vérifie si la page est inscriptible.
    #[inline(always)]
    pub const fn is_writable(self) -> bool {
        self.contains(Self::WRITABLE)
    }

    /// Vérifie si la page est accessible depuis userspace.
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.contains(Self::USER)
    }

    /// Vérifie si la page est exécutable (NX non positionné).
    #[inline(always)]
    pub const fn is_executable(self) -> bool {
        !self.contains(Self::NO_EXECUTE)
    }

    /// Vérifie s'il s'agit d'une huge page.
    #[inline(always)]
    pub const fn is_huge(self) -> bool {
        self.contains(Self::HUGE_PAGE)
    }

    /// Vérifie si la page est CoW.
    #[inline(always)]
    pub const fn is_cow(self) -> bool {
        self.contains(Self::COW)
    }

    /// Vérifie si la page est verrouillée en RAM.
    #[inline(always)]
    pub const fn is_pinned(self) -> bool {
        self.contains(Self::PINNED)
    }
}

impl BitAnd for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn bitand(self, rhs: Self) -> Self {
        PageFlags(self.0 & rhs.0)
    }
}

impl BitOr for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        PageFlags(self.0 | rhs.0)
    }
}

impl Not for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn not(self) -> Self {
        PageFlags(!self.0)
    }
}

impl fmt::Debug for PageFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageFlags(")?;
        let mut first = true;
        let flags = [
            (Self::PRESENT, "PRESENT"),
            (Self::WRITABLE, "WRITABLE"),
            (Self::USER, "USER"),
            (Self::WRITE_THROUGH, "WRITE_THROUGH"),
            (Self::NO_CACHE, "NO_CACHE"),
            (Self::ACCESSED, "ACCESSED"),
            (Self::DIRTY, "DIRTY"),
            (Self::HUGE_PAGE, "HUGE"),
            (Self::GLOBAL, "GLOBAL"),
            (Self::NO_EXECUTE, "NX"),
            (Self::COW, "COW"),
            (Self::PINNED, "PINNED"),
            (Self::SHARED, "SHARED"),
            (Self::DMA, "DMA"),
        ];
        for (flag, name) in &flags {
            if self.contains(*flag) {
                if !first {
                    write!(f, "|")?;
                }
                write!(f, "{}", name)?;
                first = false;
            }
        }
        if first {
            write!(f, "EMPTY")?;
        }
        write!(f, ")")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZONE TYPE — Catégorie de zone mémoire
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorie de zone mémoire physique.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum ZoneType {
    /// Zone DMA — <16 MiB, pour les devices 32 bits legacy.
    Dma = 0,
    /// Zone DMA32 — <4 GiB, pour les devices PCIe 32 bits.
    Dma32 = 1,
    /// Zone NORMAL — RAM principale (>= 4 GiB sur 64 bits, >= 896 MiB sur 32 bits).
    Normal = 2,
    /// Zone HIGH — Mémoire haute (32 bits uniquement, >896 MiB).
    High = 3,
    /// Zone MOVABLE — Pages déplaçables pour défragmenter les huge pages.
    Movable = 4,
}

impl ZoneType {
    /// Retourne le nombre total de zones connues.
    pub const COUNT: usize = 5;

    /// Retourne l'index numérique de la zone.
    #[inline(always)]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Convertit un index en ZoneType (None si invalide).
    #[inline(always)]
    pub const fn from_index(idx: usize) -> Option<ZoneType> {
        match idx {
            0 => Some(ZoneType::Dma),
            1 => Some(ZoneType::Dma32),
            2 => Some(ZoneType::Normal),
            3 => Some(ZoneType::High),
            4 => Some(ZoneType::Movable),
            _ => None,
        }
    }

    /// Détermine la zone appropriée pour une adresse physique donnée.
    #[inline]
    pub fn for_phys_addr(addr: PhysAddr) -> ZoneType {
        use super::constants::{ZONE_DMA32_END, ZONE_DMA_END};
        let a = addr.as_usize();
        if a < ZONE_DMA_END {
            ZoneType::Dma
        } else if a < ZONE_DMA32_END {
            ZoneType::Dma32
        } else {
            ZoneType::Normal
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOC FLAGS — Options d'allocation
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux contrôlant le comportement de l'allocateur physique.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct AllocFlags(u32);

impl AllocFlags {
    /// Aucun flag spécial.
    pub const NONE: AllocFlags = AllocFlags(0);
    /// Ne pas bloquer si la mémoire est épuisée — retourne Err immédiatement.
    pub const NO_WAIT: AllocFlags = AllocFlags(1 << 0);
    /// Allocation pour DMA (zone DMA < 16 MiB obligatoire).
    pub const DMA: AllocFlags = AllocFlags(1 << 1);
    /// Allocation DMA32 (zone DMA32 < 4 GiB).
    pub const DMA32: AllocFlags = AllocFlags(1 << 2);
    /// Initialiser les pages à zéro avant de les retourner.
    pub const ZEROED: AllocFlags = AllocFlags(1 << 3);
    /// Allocation urgente (utiliser le pool d'urgence si nécessaire).
    pub const EMERGENCY: AllocFlags = AllocFlags(1 << 4);
    /// Exécuter depuis un contexte atomic (pas de blocage autorisé).
    pub const ATOMIC: AllocFlags = AllocFlags(Self::NO_WAIT.0 | 1 << 5);
    /// Page de garde (ne jamais mapper en cache de TLB global).
    pub const GUARD: AllocFlags = AllocFlags(1 << 6);
    /// Allocation MOVABLE (eligible à la migration inter-nœuds).
    pub const MOVABLE: AllocFlags = AllocFlags(1 << 7);
    /// Verrouiller la page en RAM (ne pas permettre le swap).
    pub const PIN: AllocFlags = AllocFlags(1 << 8);

    #[inline(always)]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn contains(self, f: AllocFlags) -> bool {
        (self.0 & f.0) == f.0
    }

    #[inline(always)]
    pub const fn set(self, f: AllocFlags) -> Self {
        AllocFlags(self.0 | f.0)
    }

    /// Zone requise par ces flags.
    #[inline]
    pub const fn required_zone(self) -> ZoneType {
        if self.contains(Self::DMA) {
            ZoneType::Dma
        } else if self.contains(Self::DMA32) {
            ZoneType::Dma32
        } else if self.contains(Self::MOVABLE) {
            ZoneType::Movable
        } else {
            ZoneType::Normal
        }
    }
}

impl BitOr for AllocFlags {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        AllocFlags(self.0 | rhs.0)
    }
}

impl fmt::Debug for AllocFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AllocFlags(0b{:016b})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOC ERROR — Erreurs d'allocation
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs pouvant survenir lors d'une allocation mémoire.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AllocError {
    /// Plus aucune mémoire libre dans la zone requise.
    OutOfMemory,
    /// Fragmentation : pas de bloc contigu suffisamment grand.
    Fragmentation,
    /// Paramètres invalides (ordre nul, order trop grand, align invalide).
    InvalidParams,
    /// L'allocateur n'est pas encore initialisé.
    NotInitialized,
    /// L'allocation aurait bloqué mais NO_WAIT était positionné.
    WouldBlock,
    /// Zone mémoire demandée inexistante ou non initialisée.
    ZoneUnavailable,
    /// Dépassement de la limite de mémoire pour ce processus (cgroup).
    LimitExceeded,
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocError::OutOfMemory => write!(f, "Out of memory"),
            AllocError::Fragmentation => write!(f, "Memory fragmentation"),
            AllocError::InvalidParams => write!(f, "Invalid allocation parameters"),
            AllocError::NotInitialized => write!(f, "Allocator not initialized"),
            AllocError::WouldBlock => write!(f, "Allocation would block (NO_WAIT set)"),
            AllocError::ZoneUnavailable => write!(f, "Memory zone unavailable"),
            AllocError::LimitExceeded => write!(f, "Memory limit exceeded"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS STATIQUES SUR LES TAILLES
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    core::mem::size_of::<PhysAddr>() == 8,
    "PhysAddr doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<VirtAddr>() == 8,
    "VirtAddr doit faire 8 bytes"
);
const _: () = assert!(core::mem::size_of::<Page>() == 8, "Page doit faire 8 bytes");
const _: () = assert!(
    core::mem::size_of::<Frame>() == 8,
    "Frame doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<PageFlags>() == 8,
    "PageFlags doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<AllocFlags>() == 4,
    "AllocFlags doit faire 4 bytes"
);
const _: () = assert!(
    core::mem::size_of::<ZoneType>() == 1,
    "ZoneType doit faire 1 byte"
);
