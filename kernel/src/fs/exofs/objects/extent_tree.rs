// kernel/src/fs/exofs/objects/extent_tree.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExtentTree — arbre des extents d'un objet (mapping logique→physique)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'ExtentTree stocke la liste ordonnée des ObjectExtents d'un objet.
// Pour les petits objets (< 8 extents), on utilise un tableau statique.
// Pour les grands objets, on spille dans un Vec heap (règle OOM-02).
//
// RÈGLE ARITH-01 : checked operations sur tous les offsets.
// RÈGLE OOM-02   : try_reserve avant push.

use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, DiskOffset,
};
use crate::fs::exofs::objects::extent::ObjectExtent;

// ─────────────────────────────────────────────────────────────────────────────
// Constante
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal d'extents inline (avant spill dans le Vec heap).
const INLINE_EXTENT_COUNT: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// ExtentTree
// ─────────────────────────────────────────────────────────────────────────────

/// Arbre des extents d'un objet.
///
/// Optimisé pour les petits objets (≤ 8 extents) via stockage inline.
pub struct ExtentTree {
    /// Extents inline (évite une allocation heap pour les petits objets).
    inline_extents: [Option<ObjectExtent>; INLINE_EXTENT_COUNT],
    /// Extents supplémentaires (spill).
    spill:          Vec<ObjectExtent>,
    /// Nombre total d'extents.
    count:           usize,
}

impl ExtentTree {
    /// Crée un ExtentTree vide.
    pub fn new() -> Self {
        Self {
            inline_extents: [None; INLINE_EXTENT_COUNT],
            spill:          Vec::new(),
            count:          0,
        }
    }

    /// Ajoute un extent.
    ///
    /// RÈGLE OOM-02 : try_reserve avant push.
    pub fn push(&mut self, extent: ObjectExtent) -> ExofsResult<()> {
        if self.count < INLINE_EXTENT_COUNT {
            self.inline_extents[self.count] = Some(extent);
            self.count += 1;
        } else {
            self.spill.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            self.spill.push(extent);
            self.count += 1;
        }
        Ok(())
    }

    /// Retourne l'extent couvrant l'offset logique `offset`, si existant.
    pub fn find_extent_for_offset(&self, offset: u64) -> Option<&ObjectExtent> {
        for i in 0..self.count.min(INLINE_EXTENT_COUNT) {
            if let Some(ref ext) = self.inline_extents[i] {
                let end = ext.logical_offset.saturating_add(ext.physical.len);
                if ext.logical_offset <= offset && offset < end {
                    return Some(ext);
                }
            }
        }
        for ext in &self.spill {
            let end = ext.logical_offset.saturating_add(ext.physical.len);
            if ext.logical_offset <= offset && offset < end {
                return Some(ext);
            }
        }
        None
    }

    /// Itère sur tous les extents dans l'ordre logique.
    pub fn iter(&self) -> impl Iterator<Item = &ObjectExtent> {
        let inline_iter = self.inline_extents[..self.count.min(INLINE_EXTENT_COUNT)]
            .iter()
            .flatten();
        let spill_iter = self.spill.iter();
        inline_iter.chain(spill_iter)
    }

    /// Nombre d'extents.
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Vrai si l'arbre est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Taille totale des données couvertes par tous les extents.
    pub fn total_data_size(&self) -> u64 {
        self.iter().map(|e| e.physical.len).fold(0u64, u64::saturating_add)
    }
}
