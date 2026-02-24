// kernel/src/memory/virtual/vma/cow.rs
//
// Copy-on-Write des VMAs — gestion du bris de CoW et de l'héritage au fork.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::core::{
    VirtAddr, Frame, PhysAddr, PageFlags, AllocFlags, AllocError, PAGE_SIZE,
};
use crate::memory::physical::frame::ref_count::{AtomicRefCount, RefCountDecResult};
use super::descriptor::{VmaDescriptor, VmaFlags};

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES COW
// ─────────────────────────────────────────────────────────────────────────────

pub struct CowStats {
    pub breaks:       AtomicU64,
    pub shared_pages: AtomicU64,
    pub zero_pages:   AtomicU64,
    pub fork_copies:  AtomicU64,
}

impl CowStats {
    pub const fn new() -> Self {
        CowStats {
            breaks:       AtomicU64::new(0),
            shared_pages: AtomicU64::new(0),
            zero_pages:   AtomicU64::new(0),
            fork_copies:  AtomicU64::new(0),
        }
    }
}

pub static COW_STATS: CowStats = CowStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT D'ALLOCATION POUR COW
// ─────────────────────────────────────────────────────────────────────────────

/// Trait permettant au module CoW d'allouer/libérer des frames.
pub trait CowFrameAllocator: Sync {
    fn alloc_zeroed(&self)     -> Result<Frame, AllocError>;
    fn alloc_nonzeroed(&self)  -> Result<Frame, AllocError>;
    fn free_frame(&self, f: Frame);
}

// ─────────────────────────────────────────────────────────────────────────────
// BRIS DE COW (CoW break)
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un bris de CoW pour une page.
pub enum CowBreakResult {
    /// La page avait refcount=1 (exclusive) — il n'y a rien à copier,
    /// il suffit de marquer la page en écriture.
    AlreadyExclusive(Frame),
    /// Une copie a été réalisée — nouveau frame.
    Copied(Frame),
    /// Le frame source était la zero_page — allouer une page zéro.
    ZeroPage(Frame),
    /// Erreur d'allocation.
    Error(AllocError),
}

/// Effectue un bris de CoW pour `old_frame`.
///
/// SAFETY: `old_frame` doit avoir été préalablement pin pour éviter la
///         libération concurrente. Le TLB doit être invalidé par l'appelant.
pub unsafe fn cow_break<A: CowFrameAllocator>(
    old_frame: Frame,
    refcount:  &AtomicRefCount,
    alloc:     &A,
) -> CowBreakResult {
    // Tenter de décrémenter : si refcount passe à 1, le frame devient exclusif.
    match refcount.dec() {
        RefCountDecResult::BecameExclusive => {
            // Le frame est maintenant exclusif — plus de CoW, juste remettre en W.
            COW_STATS.breaks.fetch_add(1, Ordering::Relaxed);
            CowBreakResult::AlreadyExclusive(old_frame)
        }
        RefCountDecResult::StillShared => {
            // Shared : allouer un nouveau frame et copier le contenu.
            let new_frame = match alloc.alloc_nonzeroed() {
                Ok(f)  => f,
                Err(e) => {
                    // Remettre le refcount à son état précédent.
                    refcount.inc();
                    return CowBreakResult::Error(e);
                }
            };
            // Copier le contenu de l'ancien frame vers le nouveau.
            let src = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
                       + old_frame.start_address().as_u64()) as *const u8;
            let dst = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
                       + new_frame.start_address().as_u64()) as *mut u8;
            // SAFETY: src et dst sont des plages physiques valides mappées
            //         dans le physmap. Taille = PAGE_SIZE octets.
            core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
            COW_STATS.breaks.fetch_add(1, Ordering::Relaxed);
            COW_STATS.fork_copies.fetch_add(1, Ordering::Relaxed);
            CowBreakResult::Copied(new_frame)
        }
        RefCountDecResult::ShouldFree => {
            // Dernier référent — libérer le frame et prendre le nouveau.
            // (Ce cas ne devrait pas arriver si on break sur refcount >= 2,
            //  mais on le gère pour la robustesse.)
            alloc.free_frame(old_frame);
            match alloc.alloc_zeroed() {
                Ok(f)  => {
                    COW_STATS.zero_pages.fetch_add(1, Ordering::Relaxed);
                    CowBreakResult::ZeroPage(f)
                }
                Err(e) => CowBreakResult::Error(e),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FORK : MARQUER UNE VMA COMME COW
// ─────────────────────────────────────────────────────────────────────────────

/// Lors d'un fork, marquer une VMA comme CoW (désactiver l'écriture dans
/// les tables de pages parent et enfant — le bris aura lieu au premier write).
///
/// - Retire le flag WRITABLE des entrées PT parent.
/// - Ajoute le flag COW aux entrées PT parent.
/// - Le child héritera des mêmes entrées (copie de PML4 faite ailleurs).
pub fn mark_vma_cow(vma: &mut VmaDescriptor) {
    // La VMA n'est modifiable en écriture que via le trait PageTableWalker
    // qui est appellé par le gestionnaire de fault. Ici on marque uniquement
    // les flags du descripteur — le walk de table sera fait par address_space::fork().
    if vma.flags.contains(VmaFlags::WRITE) && !vma.flags.contains(VmaFlags::SHARED) {
        vma.flags    |= VmaFlags::COW;
        vma.page_flags = vma.page_flags & !PageFlags::WRITABLE | PageFlags::COW;
    }
    COW_STATS.shared_pages.fetch_add(vma.n_pages() as u64, Ordering::Relaxed);
}
