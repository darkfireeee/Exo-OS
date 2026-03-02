// kernel/src/memory/virtual/vma/operations.rs
//
// Opérations sur les VMAs : mmap, munmap, mprotect, split, merge.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{VirtAddr, PageFlags, AllocError, PAGE_SIZE};
use super::descriptor::{VmaDescriptor, VmaFlags, VmaBacking};
use super::tree::VmaTree;

// ─────────────────────────────────────────────────────────────────────────────
// PARAMÈTRES D'ALLOCATION VMA
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres pour créer une nouvelle VMA.
#[derive(Debug, Clone)]
pub struct VmaAllocParams {
    pub hint_addr:   Option<VirtAddr>,  // Adresse souhaitée (peut être ignorée)
    pub size:        usize,
    pub flags:       VmaFlags,
    pub page_flags:  PageFlags,
    pub backing:     VmaBacking,
    pub inode_id:    u64,
    pub file_offset: u64,
    pub fixed:       bool,              // MAP_FIXED : utiliser hint_addr exactement
}

impl VmaAllocParams {
    pub fn anonymous(size: usize, flags: VmaFlags, page_flags: PageFlags) -> Self {
        VmaAllocParams {
            hint_addr:   None,
            size,
            flags:       flags | VmaFlags::ANONYMOUS,
            page_flags,
            backing:     VmaBacking::Anonymous,
            inode_id:    0,
            file_offset: 0,
            fixed:       false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GESTION DES GAPS (plages libres)
// ─────────────────────────────────────────────────────────────────────────────

/// Recherche un gap disponible dans l'arbre pour une VMA de `size` octets.
///
/// Stratégie : first-fit en partant de `hint_start`, puis depuis `min_addr`
/// si non trouvé.
pub fn find_gap(
    tree:       &VmaTree,
    size:       usize,
    hint_start: VirtAddr,
    min_addr:   VirtAddr,
    max_addr:   VirtAddr,
) -> Option<VirtAddr> {
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Clamp hint
    let start_hint = if hint_start.as_u64() >= min_addr.as_u64() && hint_start.as_u64() < max_addr.as_u64() {
        hint_start
    } else {
        min_addr
    };

    // Essai à partir du hint
    if let Some(addr) = scan_gap(tree, start_hint, max_addr, aligned_size) {
        return Some(addr);
    }
    // Essai depuis min_addr
    if start_hint.as_u64() > min_addr.as_u64() {
        return scan_gap(tree, min_addr, start_hint, aligned_size);
    }
    None
}

fn scan_gap(
    tree:      &VmaTree,
    from:      VirtAddr,
    to:        VirtAddr,
    size:      usize,
) -> Option<VirtAddr> {
    let mut cursor = align_up_addr(from, PAGE_SIZE);
    for vma in tree.iter() {
        // Gap entre cursor et début de cette VMA
        if vma.start.as_u64() > cursor.as_u64() {
            let gap = (vma.start.as_u64() - cursor.as_u64()) as usize;
            if gap >= size && cursor.as_u64() + size as u64 <= to.as_u64() {
                return Some(cursor);
            }
        }
        // Avancer après la fin de cette VMA
        if vma.end.as_u64() > cursor.as_u64() {
            cursor = VirtAddr::new(vma.end.as_u64());
        }
    }
    // Gap après toutes les VMAs
    if cursor.as_u64() + size as u64 <= to.as_u64() {
        return Some(cursor);
    }
    None
}

fn align_up_addr(addr: VirtAddr, align: usize) -> VirtAddr {
    VirtAddr::new((addr.as_u64() + align as u64 - 1) & !(align as u64 - 1))
}

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT D'UNE VMA
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un split de VMA.
pub struct SplitResult {
    /// Partie gauche (start..split_point).
    pub left:  VmaDescriptor,
    /// Partie droite (split_point..end).
    pub right: VmaDescriptor,
}

/// Divise `vma` en deux à l'adresse `split_point`.
///
/// `split_point` doit être aligné sur PAGE_SIZE et dans [start..end).
pub fn split_vma(vma: &VmaDescriptor, split_point: VirtAddr) -> Result<SplitResult, AllocError> {
    let sp = split_point.as_u64();
    if sp <= vma.start.as_u64() || sp >= vma.end.as_u64() {
        return Err(AllocError::InvalidParams);
    }
    if sp % PAGE_SIZE as u64 != 0 {
        return Err(AllocError::InvalidParams);
    }

    let mut left = VmaDescriptor::new(
        vma.start, split_point,
        vma.flags, vma.page_flags, vma.backing,
    );
    // Propager l'association fichier : left part depuis vma.start, donc
    // son file_offset est identique à celui de la VMA d'origine.
    left.inode_id    = vma.inode_id;
    left.file_offset = vma.file_offset;

    let mut right = VmaDescriptor::new(
        split_point, vma.end,
        vma.flags, vma.page_flags, vma.backing,
    );
    right.inode_id    = vma.inode_id;
    right.file_offset = vma.file_offset + (sp - vma.start.as_u64());

    Ok(SplitResult { left, right })
}

// ─────────────────────────────────────────────────────────────────────────────
// MPROTECT
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de mprotect : VMAs à insérer après la transformation.
pub struct MprotectResult {
    /// VMA avant la plage modifiée (peut être None si start == vma.start).
    pub before:  Option<VmaDescriptor>,
    /// VMA modifiée (la plage [start..end) avec les nouveaux flags).
    pub middle:  VmaDescriptor,
    /// VMA après la plage modifiée (peut être None si end == vma.end).
    pub after:   Option<VmaDescriptor>,
}

/// Applique une modification de flags sur la plage [start..end) d'une VMA.
pub fn mprotect_vma(
    vma:       &VmaDescriptor,
    start:     VirtAddr,
    end:       VirtAddr,
    new_flags: PageFlags,
) -> Result<MprotectResult, AllocError> {
    if start.as_u64() < vma.start.as_u64() || end.as_u64() > vma.end.as_u64() {
        return Err(AllocError::InvalidParams);
    }

    // Calculer les offsets fichier pour chaque fragment.
    let before = if start.as_u64() > vma.start.as_u64() {
        let mut d = VmaDescriptor::new(vma.start, start, vma.flags, vma.page_flags, vma.backing);
        d.inode_id    = vma.inode_id;
        d.file_offset = vma.file_offset;
        Some(d)
    } else {
        None
    };

    let mut middle = VmaDescriptor::new(start, end, vma.flags, new_flags, vma.backing);
    middle.inode_id    = vma.inode_id;
    middle.file_offset = vma.file_offset + (start.as_u64() - vma.start.as_u64());

    let after = if end.as_u64() < vma.end.as_u64() {
        let mut d = VmaDescriptor::new(end, vma.end, vma.flags, vma.page_flags, vma.backing);
        d.inode_id    = vma.inode_id;
        d.file_offset = vma.file_offset + (end.as_u64() - vma.start.as_u64());
        Some(d)
    } else {
        None
    };

    Ok(MprotectResult { before, middle, after })
}

// ─────────────────────────────────────────────────────────────────────────────
// VALIDATION DE VMAS
// ─────────────────────────────────────────────────────────────────────────────

/// Valide la cohérence d'un VmaDescriptor avant insertion.
pub fn validate_vma(vma: &VmaDescriptor) -> Result<(), AllocError> {
    let start = vma.start.as_u64();
    let end   = vma.end.as_u64();
    if start >= end                       { return Err(AllocError::InvalidParams); }
    if start % PAGE_SIZE as u64 != 0     { return Err(AllocError::InvalidParams); }
    if end   % PAGE_SIZE as u64 != 0     { return Err(AllocError::InvalidParams); }
    if vma.size() == 0                    { return Err(AllocError::InvalidParams); }
    Ok(())
}
