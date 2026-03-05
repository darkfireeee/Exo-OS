// kernel/src/memory/virtual/vma/descriptor.rs
//
// Descripteur de VMA (Virtual Memory Area).
// Représente une région de l'espace d'adressage virtuel d'un processus.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::memory::core::{VirtAddr, PageFlags, VirtRange};

// ─────────────────────────────────────────────────────────────────────────────
// FLAGS DE VMA
// ─────────────────────────────────────────────────────────────────────────────

/// Flags décrivant le type et les propriétés d'une VMA.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct VmaFlags(u32);

impl VmaFlags {
    pub const NONE:        VmaFlags = VmaFlags(0);
    pub const READ:        VmaFlags = VmaFlags(1 << 0);
    pub const WRITE:       VmaFlags = VmaFlags(1 << 1);
    pub const EXEC:        VmaFlags = VmaFlags(1 << 2);
    pub const SHARED:      VmaFlags = VmaFlags(1 << 3);
    pub const ANONYMOUS:   VmaFlags = VmaFlags(1 << 4);
    pub const STACK:       VmaFlags = VmaFlags(1 << 5);
    pub const HEAP:        VmaFlags = VmaFlags(1 << 6);
    pub const FIXED:       VmaFlags = VmaFlags(1 << 7);  // Adresse fixée (MAP_FIXED)
    pub const GROWSDOWN:   VmaFlags = VmaFlags(1 << 8);  // Pile qui croît vers le bas
    pub const COW:         VmaFlags = VmaFlags(1 << 9);  // Copy-on-Write actif
    pub const LOCKED:      VmaFlags = VmaFlags(1 << 10); // mlock
    pub const HUGETLB:     VmaFlags = VmaFlags(1 << 11); // Huge pages TLB
    pub const IO:          VmaFlags = VmaFlags(1 << 12); // Mapping I/O (non-cacheable)
    pub const KERNEL:      VmaFlags = VmaFlags(1 << 13); // VMA kernel
    pub const DONTEXPAND:  VmaFlags = VmaFlags(1 << 14); // Non-expandable
    pub const WIPEONFORK:  VmaFlags = VmaFlags(1 << 15); // Effacer au fork
    pub const DONTCOPY:    VmaFlags = VmaFlags(1 << 16); // Non copié au fork (SignalTcb)

    pub const fn contains(self, other: VmaFlags) -> bool { (self.0 & other.0) == other.0 }
    pub const fn bits(self) -> u32 { self.0 }
    /// Construit un VmaFlags depuis une valeur brute.
    pub const fn from_bits(v: u32) -> Self { VmaFlags(v) }
}

impl core::ops::BitOr for VmaFlags {
    type Output = VmaFlags;
    fn bitor(self, other: VmaFlags) -> VmaFlags { VmaFlags(self.0 | other.0) }
}

impl core::ops::BitOrAssign for VmaFlags {
    fn bitor_assign(&mut self, other: VmaFlags) { self.0 |= other.0; }
}

impl core::fmt::Debug for VmaFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VmaFlags({:#010x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TYPE DE BACKING (source des pages)
// ─────────────────────────────────────────────────────────────────────────────

/// Source des pages d'une VMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VmaBacking {
    /// Pages anonymes (zéro-fill à la demande).
    Anonymous = 0,
    /// Fichier mappé en mémoire (file-backed).
    File      = 1,
    /// Mapping de périphérique (device memory).
    Device    = 2,
    /// Pages kernel directement mappées (pas de demand paging).
    Direct    = 3,
    /// Segment partagé (IPC shared memory).
    Shared    = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTEUR DE VMA
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur complet d'une VMA.
/// Cette structure est la seule source de vérité pour les propriétés d'une région.
///
/// Allouée dans le slab allocator (< 2 KiB).
#[repr(C, align(64))]
pub struct VmaDescriptor {
    // ── Limites virtuelles ──────────────────────────────────────────────────
    /// Début de la région (inclus, aligné sur PAGE_SIZE).
    pub start: VirtAddr,
    /// Fin de la région (exclus, aligné sur PAGE_SIZE).
    pub end:   VirtAddr,

    // ── Propriétés ──────────────────────────────────────────────────────────
    pub flags:         VmaFlags,
    pub page_flags:    PageFlags,
    pub backing:       VmaBacking,

    // ── Fichier backing (si VmaBacking::File) ───────────────────────────────
    /// Inode ID (0 = pas de fichier).
    pub inode_id:      u64,
    /// Offset dans le fichier.
    pub file_offset:   u64,

    // ── Statistiques ────────────────────────────────────────────────────────
    pub pages_resident:  AtomicU32,
    pub pages_swapped:   AtomicU32,
    pub page_faults:     AtomicU64,
    pub cow_breaks:      AtomicU32,

    // ── Arbre AVL (liens internes) ───────────────────────────────────────────
    /// Prochain nœud (ordre adresse croissante).
    pub(crate) rb_left:  *mut VmaDescriptor,
    pub(crate) rb_right: *mut VmaDescriptor,
    pub(crate) rb_height: i32,

    pub _pad: [u8; 4],
}

// SAFETY: VmaDescriptor est un RON (Record Of Non-moveable), protégé par
//         le verrou de l'address space parent.
unsafe impl Send for VmaDescriptor {}
unsafe impl Sync for VmaDescriptor {}

impl VmaDescriptor {
    /// Crée un descripteur VMA avec les paramètres de base.
    pub const fn new(
        start:      VirtAddr,
        end:        VirtAddr,
        flags:      VmaFlags,
        page_flags: PageFlags,
        backing:    VmaBacking,
    ) -> Self {
        VmaDescriptor {
            start, end, flags, page_flags, backing,
            inode_id:    0,
            file_offset: 0,
            pages_resident: AtomicU32::new(0),
            pages_swapped:  AtomicU32::new(0),
            page_faults:    AtomicU64::new(0),
            cow_breaks:     AtomicU32::new(0),
            rb_left:  core::ptr::null_mut(),
            rb_right: core::ptr::null_mut(),
            rb_height: 1,
            _pad: [0; 4],
        }
    }

    /// Retourne la taille en octets de la VMA.
    #[inline] pub fn size(&self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Retourne le nombre de pages de la VMA.
    #[inline] pub fn n_pages(&self) -> usize {
        self.size() / crate::memory::core::PAGE_SIZE
    }

    /// Vérifie si l'adresse virtuelle est dans cette VMA.
    #[inline] pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Retourne la plage virtuelle de cette VMA.
    #[inline] pub fn range(&self) -> VirtRange {
        VirtRange::from_range(self.start, self.end)
    }

    /// Incrémente le compteur de page faults.
    #[inline] pub fn record_fault(&self) {
        self.page_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrémente le compteur de CoW breaks.
    #[inline] pub fn record_cow_break(&self) {
        self.cow_breaks.fetch_add(1, Ordering::Relaxed);
    }

    /// Vérifie si cette VMA peut être fusionnée avec `other` (adjacence + même flags).
    pub fn can_merge_with(&self, other: &VmaDescriptor) -> bool {
        self.end == other.start &&
        self.flags      == other.flags &&
        self.page_flags == other.page_flags &&
        self.backing    == other.backing &&
        self.inode_id   == other.inode_id
    }
}

impl core::fmt::Debug for VmaDescriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f,
            "VMA[{:#x}..{:#x} flags={:?} backing={:?}]",
            self.start.as_u64(), self.end.as_u64(), self.flags, self.backing
        )
    }
}
