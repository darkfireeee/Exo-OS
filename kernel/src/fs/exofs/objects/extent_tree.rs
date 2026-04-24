// SPDX-License-Identifier: MIT
// ExoFS — extent_tree.rs
// Arbre des extents d'un objet : mapping logique → physique.
// Règles :
//   ARITH-02  : checked_add / saturating_* pour tout calcul d'offset
//   OOM-02    : try_reserve(1) avant tout push
//   RECUR-01  : itératif uniquement, jamais de récursion

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::objects::extent::{ObjectExtent, ObjectExtentDisk};

// ── Constantes ─────────────────────────────────────────────────────────────────

/// Nombre maximal d'extents stockés inline (sans allocation heap).
pub const INLINE_EXTENT_COUNT: usize = 8;

/// Nombre maximal total d'extents par objet (limite de sécurité).
pub const EXTENT_MAX_COUNT: usize = 65536;

// ── ExtentTree ─────────────────────────────────────────────────────────────────

/// Arbre des extents d'un `LogicalObject`.
///
/// Optimisé pour le cas courant (≤ 8 extents) avec stockage inline évitant
/// toute allocation heap. Dès que le 9ème extent est ajouté, les entrées
/// débordent dans un `Vec` heap.
///
/// Invariant maintenu : les extents sont toujours triés par `logical_offset`
/// croissant et ne se chevauchent pas.
pub struct ExtentTree {
    /// Extents inline (slots initialisés à `None` = libre).
    inline_extents: [Option<ObjectExtent>; INLINE_EXTENT_COUNT],
    /// Extents supplémentaires au-delà de `INLINE_EXTENT_COUNT`.
    spill: Vec<ObjectExtent>,
    /// Nombre total d'extents (inline + spill).
    count: usize,
    /// Statistiques.
    pub stats: ExtentTreeStats,
}

impl ExtentTree {
    // ── Constructeurs ────────────────────────────────────────────────────────

    /// Crée un `ExtentTree` vide.
    pub fn new() -> Self {
        Self {
            inline_extents: [None; INLINE_EXTENT_COUNT],
            spill: Vec::new(),
            count: 0,
            stats: ExtentTreeStats::new(),
        }
    }

    // ── Insertion ─────────────────────────────────────────────────────────────

    /// Insère un extent en maintenant le tri par `logical_offset`.
    ///
    /// Règle OOM-02 : `try_reserve(1)` avant tout `push` dans `spill`.
    ///
    /// Retourne `ExofsError::InvalidArgument` si l'extent est invalide ou
    /// chevauche un extent existant.
    pub fn insert(&mut self, extent: ObjectExtent) -> ExofsResult<()> {
        extent.validate()?;
        // Vérifier les chevauchements (itératif — RECUR-01).
        for e in self.iter() {
            if e.overlaps_logical(&extent) {
                return Err(ExofsError::InvalidArgument);
            }
        }
        if self.count >= EXTENT_MAX_COUNT {
            return Err(ExofsError::NoSpace);
        }

        if self.count < INLINE_EXTENT_COUNT {
            // Trouver la position d'insertion pour maintenir le tri.
            let pos = self.find_inline_insert_pos(extent.logical_offset);
            self.inline_shift_right(pos);
            self.inline_extents[pos] = Some(extent);
        } else {
            // Spill vers le Vec heap.
            self.spill
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            // Trouver la position dans le vec pour maintenir le tri.
            let pos = self.find_spill_insert_pos(extent.logical_offset);
            self.spill.insert(pos, extent);
        }
        // ARITH-02 : saturating_add (count est protégé par EXTENT_MAX_COUNT).
        self.count = self.count.saturating_add(1);
        self.stats.insert_count = self.stats.insert_count.saturating_add(1);
        Ok(())
    }

    /// Alias pour `insert` (compatibilité avec le code précédent).
    #[inline]
    pub fn push(&mut self, extent: ObjectExtent) -> ExofsResult<()> {
        self.insert(extent)
    }

    // ── Suppression ───────────────────────────────────────────────────────────

    /// Supprime l'extent qui couvre l'offset logique `offset`.
    ///
    /// Retourne l'extent supprimé, ou `ExofsError::NotFound` s'il n'existe pas.
    pub fn remove_at_offset(&mut self, offset: u64) -> ExofsResult<ObjectExtent> {
        // Chercher dans inline.
        for i in 0..self.count.min(INLINE_EXTENT_COUNT) {
            if let Some(ref e) = self.inline_extents[i] {
                if e.contains_offset(offset) {
                    let removed = e.clone();
                    self.inline_shift_left(i);
                    self.count = self.count.saturating_sub(1);
                    self.stats.remove_count = self.stats.remove_count.saturating_add(1);
                    return Ok(removed);
                }
            }
        }
        // Chercher dans le spill.
        for i in 0..self.spill.len() {
            if self.spill[i].contains_offset(offset) {
                let removed = self.spill.remove(i);
                self.count = self.count.saturating_sub(1);
                self.stats.remove_count = self.stats.remove_count.saturating_add(1);
                return Ok(removed);
            }
        }
        Err(ExofsError::NotFound)
    }

    // ── Recherche ─────────────────────────────────────────────────────────────

    /// Retourne une référence vers l'extent couvrant l'offset logique `offset`.
    ///
    /// Retourne `None` si aucun extent ne couvre cet offset (trou).
    pub fn find_extent_for_offset(&self, offset: u64) -> Option<&ObjectExtent> {
        self.stats
            .lookup_count
            .set(self.stats.lookup_count.get().saturating_add(1));
        for e in self.iter() {
            if e.contains_offset(offset) {
                self.stats
                    .lookup_hit
                    .set(self.stats.lookup_hit.get().saturating_add(1));
                return Some(e);
            }
        }
        None
    }

    /// Retourne une référence mutable vers l'extent couvrant `offset`.
    pub fn find_extent_for_offset_mut(&mut self, offset: u64) -> Option<&mut ObjectExtent> {
        // Chercher dans inline.
        for i in 0..self.count.min(INLINE_EXTENT_COUNT) {
            if let Some(ref e) = self.inline_extents[i] {
                if e.contains_offset(offset) {
                    return self.inline_extents[i].as_mut();
                }
            }
        }
        // Chercher dans spill.
        for e in &mut self.spill {
            if e.contains_offset(offset) {
                return Some(e);
            }
        }
        None
    }

    // ── Itération ─────────────────────────────────────────────────────────────

    /// Itère sur tous les extents triés par `logical_offset` croissant.
    pub fn iter(&self) -> impl Iterator<Item = &ObjectExtent> {
        let inline_count = self.count.min(INLINE_EXTENT_COUNT);
        let inline_iter = self.inline_extents[..inline_count].iter().flatten();
        let spill_iter = self.spill.iter();
        inline_iter.chain(spill_iter)
    }

    // ── Métriques ─────────────────────────────────────────────────────────────

    /// Nombre total d'extents.
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Vrai si l'arbre est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Taille totale des octets couverts par tous les extents (ARITH-02).
    pub fn total_data_size(&self) -> u64 {
        self.iter()
            .fold(0u64, |acc, e| acc.saturating_add(e.physical.len))
    }

    /// Taille totale des extents non-sparse.
    pub fn allocated_data_size(&self) -> u64 {
        self.iter()
            .filter(|e| !e.is_sparse())
            .fold(0u64, |acc, e| acc.saturating_add(e.physical.len))
    }

    /// Taille totale des extents sparse (trous).
    pub fn sparse_size(&self) -> u64 {
        self.iter()
            .filter(|e| e.is_sparse())
            .fold(0u64, |acc, e| acc.saturating_add(e.physical.len))
    }

    /// Pourcentage d'occupation (données réelles / total × 100).
    ///
    /// Retourne 100 si l'arbre est vide.
    pub fn coverage_percent(&self) -> u64 {
        let total = self.total_data_size();
        if total == 0 {
            return 100;
        }
        let alloc = self.allocated_data_size();
        alloc.saturating_mul(100).checked_div(total).unwrap_or(100)
    }

    // ── Trous (holes) ─────────────────────────────────────────────────────────

    /// Retourne le premier trou logique à partir de `start_offset`.
    ///
    /// Itératif (RECUR-01). Retourne `None` si le fichier est dense.
    pub fn find_first_hole_after(&self, start_offset: u64) -> Option<(u64, u64)> {
        let mut cursor = start_offset;
        for e in self.iter() {
            if e.logical_offset > cursor {
                // Trou entre `cursor` et `e.logical_offset`.
                return Some((cursor, e.logical_offset));
            }
            // Avancer après cet extent.
            cursor = match e.logical_end() {
                Ok(end) => end,
                Err(_) => return None,
            };
        }
        None // Pas de trou trouvé.
    }

    /// Retourne `true` si l'objet est dense (aucun trou).
    pub fn is_dense(&self) -> bool {
        self.find_first_hole_after(0).is_none()
    }

    // ── Fusion ────────────────────────────────────────────────────────────────

    /// Fusionne les extents contigus ayant les mêmes flags.
    ///
    /// Itératif (RECUR-01). Modifie l'arbre en place.
    /// Retourne le nombre de fusions effectuées.
    pub fn merge_contiguous(&mut self) -> usize {
        if self.count < 2 {
            return 0;
        }
        let mut merged: usize = 0;
        // On collecte tous les extents, fusionne, puis on reconstruit.
        let mut all: Vec<ObjectExtent> = self.iter().cloned().collect();
        let mut i = 0;
        // Iteration : on tente de fusionner all[i] avec all[i+1].
        while i + 1 < all.len() {
            let next = all[i + 1];
            if all[i].try_merge(&next).is_ok() {
                all.remove(i + 1);
                merged = merged.saturating_add(1);
                // Ne pas avancer i : on tente de fusionner encore avec le suivant.
            } else {
                i += 1;
            }
        }
        if merged > 0 {
            self.rebuild_from_slice(&all);
            self.stats.merge_count = self.stats.merge_count.saturating_add(merged as u64);
        }
        merged
    }

    // ── Tri ───────────────────────────────────────────────────────────────────

    /// Re-trie les extents par `logical_offset` croissant.
    ///
    /// Utilisé après une série d'insertions désordonnées.
    pub fn sort_by_logical_offset(&mut self) {
        let mut all: Vec<ObjectExtent> = self.iter().cloned().collect();
        // Tri par insertion (RECUR-01 : pas de tri récursif).
        // Pour de petites listes (≤ 64), le tri par insertion est optimal.
        for i in 1..all.len() {
            let mut j = i;
            while j > 0 && all[j - 1].logical_offset > all[j].logical_offset {
                all.swap(j - 1, j);
                j -= 1;
            }
        }
        self.rebuild_from_slice(&all);
    }

    // ── Serialisation / Désérialisation ───────────────────────────────────────

    /// Sérialise tous les extents vers un `Vec<ObjectExtentDisk>`.
    ///
    /// Règle OOM-02 : `try_reserve` avant push.
    pub fn to_disk_vec(&self) -> ExofsResult<Vec<ObjectExtentDisk>> {
        let mut out = Vec::new();
        out.try_reserve(self.count)
            .map_err(|_| ExofsError::NoMemory)?;
        for e in self.iter() {
            out.push(e.to_disk());
        }
        Ok(out)
    }

    /// Reconstruit l'arbre depuis un slice d'entrées on-disk.
    ///
    /// Les extents sont validés avant insertion (HDR-03 analogue).
    pub fn from_disk_slice(entries: &[ObjectExtentDisk]) -> ExofsResult<Self> {
        let mut tree = Self::new();
        for d in entries {
            let e = ObjectExtent::from_disk(*d);
            e.validate()?;
            tree.insert(e)?;
        }
        Ok(tree)
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /// Valide l'arbre entier.
    ///
    /// Checks :
    /// 1. Chaque extent individuellement valide.
    /// 2. Extents triés par `logical_offset`.
    /// 3. Pas de chevauchement logique.
    pub fn validate(&self) -> ExofsResult<()> {
        let mut prev_end: Option<u64> = None;
        for e in self.iter() {
            e.validate()?;
            let lo = e.logical_offset;
            let end = e.logical_end()?;
            if let Some(pe) = prev_end {
                if lo < pe {
                    return Err(ExofsError::Corrupt); // Chevauchement.
                }
            }
            prev_end = Some(end);
        }
        Ok(())
    }

    // ── Helpers privés ────────────────────────────────────────────────────────

    /// Trouve la position d'insertion dans `inline_extents` pour maintenir le tri.
    fn find_inline_insert_pos(&self, lo: u64) -> usize {
        let max = self.count.min(INLINE_EXTENT_COUNT);
        for i in 0..max {
            if let Some(ref e) = self.inline_extents[i] {
                if e.logical_offset > lo {
                    return i;
                }
            }
        }
        max
    }

    /// Décale les extents inline vers la droite à partir de `pos`.
    fn inline_shift_right(&mut self, pos: usize) {
        let max = self.count.min(INLINE_EXTENT_COUNT - 1);
        for i in (pos..max).rev() {
            self.inline_extents[i + 1] = self.inline_extents[i];
        }
        self.inline_extents[pos] = None;
    }

    /// Décale les extents inline vers la gauche à partir de `pos + 1`.
    fn inline_shift_left(&mut self, pos: usize) {
        let max = self.count.min(INLINE_EXTENT_COUNT) - 1;
        for i in pos..max {
            self.inline_extents[i] = self.inline_extents[i + 1];
        }
        if max < INLINE_EXTENT_COUNT {
            self.inline_extents[max] = None;
        }
    }

    /// Trouve la position d'insertion dans `spill` pour maintenir le tri.
    fn find_spill_insert_pos(&self, lo: u64) -> usize {
        for (i, e) in self.spill.iter().enumerate() {
            if e.logical_offset > lo {
                return i;
            }
        }
        self.spill.len()
    }

    /// Reconstruit l'arbre depuis un slice trié.
    fn rebuild_from_slice(&mut self, src: &[ObjectExtent]) {
        self.inline_extents = [None; INLINE_EXTENT_COUNT];
        self.spill.clear();
        self.count = 0;
        let n = src.len().min(INLINE_EXTENT_COUNT);
        for (i, e) in src[..n].iter().enumerate() {
            self.inline_extents[i] = Some(*e);
        }
        if src.len() > INLINE_EXTENT_COUNT {
            self.spill.extend_from_slice(&src[INLINE_EXTENT_COUNT..]);
        }
        self.count = src.len();
    }
}

impl Default for ExtentTree {
    fn default() -> Self {
        Self::new()
    }
}

// ── Display / Debug ────────────────────────────────────────────────────────────

impl fmt::Display for ExtentTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExtentTree {{ count: {}, total_size: {}, coverage: {}%, \
             sparse: {}, stats: {} }}",
            self.count,
            self.total_data_size(),
            self.coverage_percent(),
            self.sparse_size(),
            self.stats,
        )
    }
}

impl fmt::Debug for ExtentTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── ExtentTreeStats ────────────────────────────────────────────────────────────

/// Statistiques sur les opérations de l'`ExtentTree`.
///
/// Utilisation de `core::cell::Cell` pour la mutabilité intérieure
/// sur `lookup_count` et `lookup_hit` (appelées depuis `&self`).
pub struct ExtentTreeStats {
    /// Nombre d'insertions.
    pub insert_count: u64,
    /// Nombre de suppressions.
    pub remove_count: u64,
    /// Nombre de fusions.
    pub merge_count: u64,
    /// Nombre de recherches.
    lookup_count: core::cell::Cell<u64>,
    /// Nombre de recherches ayant abouti.
    lookup_hit: core::cell::Cell<u64>,
    /// Nombre d'erreurs de validation.
    pub validate_err: u64,
}

impl ExtentTreeStats {
    pub const fn new() -> Self {
        Self {
            insert_count: 0,
            remove_count: 0,
            merge_count: 0,
            lookup_count: core::cell::Cell::new(0),
            lookup_hit: core::cell::Cell::new(0),
            validate_err: 0,
        }
    }

    /// Nombre total de recherches.
    pub fn lookups(&self) -> u64 {
        self.lookup_count.get()
    }

    /// Nombre de recherches réussies.
    pub fn hits(&self) -> u64 {
        self.lookup_hit.get()
    }

    /// Taux de succès des recherches (×100).
    pub fn hit_rate_x100(&self) -> u64 {
        let total = self.lookup_count.get();
        if total == 0 {
            return 100;
        }
        self.lookup_hit
            .get()
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
    }
}

impl Default for ExtentTreeStats {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ExtentTreeStats {
    fn clone(&self) -> Self {
        Self {
            insert_count: self.insert_count,
            remove_count: self.remove_count,
            merge_count: self.merge_count,
            lookup_count: core::cell::Cell::new(self.lookup_count.get()),
            lookup_hit: core::cell::Cell::new(self.lookup_hit.get()),
            validate_err: self.validate_err,
        }
    }
}

impl fmt::Debug for ExtentTreeStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ExtentTreeStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExtentTreeStats {{ inserts: {}, removes: {}, merges: {}, \
             lookups: {}, hit_rate: {}% }}",
            self.insert_count,
            self.remove_count,
            self.merge_count,
            self.lookup_count.get(),
            self.hit_rate_x100(),
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::DiskOffset;
    use crate::fs::exofs::objects::extent::EXTENT_FLAG_SPARSE;

    fn mk_extent(lo: u64, len: u64) -> ObjectExtent {
        ObjectExtent::new(lo, DiskOffset(lo + 0x10000), len, 0)
    }

    fn mk_sparse(lo: u64, len: u64) -> ObjectExtent {
        ObjectExtent::new(lo, DiskOffset(0), len, EXTENT_FLAG_SPARSE)
    }

    #[test]
    fn test_insert_and_find() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x1000)).unwrap();
        tree.insert(mk_extent(0x1000, 0x1000)).unwrap();
        tree.insert(mk_extent(0x2000, 0x1000)).unwrap();

        assert!(tree.find_extent_for_offset(0x500).is_some());
        assert!(tree.find_extent_for_offset(0x1800).is_some());
        assert!(tree.find_extent_for_offset(0x3000).is_none());
    }

    #[test]
    fn test_overlap_rejected() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x2000)).unwrap();
        // Chevauchement : doit échouer.
        assert!(tree.insert(mk_extent(0x1000, 0x1000)).is_err());
    }

    #[test]
    fn test_merge_contiguous() {
        let mut tree = ExtentTree::new();
        for lo in (0u64..8).map(|i| i * 0x1000) {
            tree.insert(mk_extent(lo, 0x1000)).unwrap();
        }
        let n = tree.merge_contiguous();
        // Tous les extents contigus ont été fusionnés en un seul.
        assert!(n > 0);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_spill_over_inline() {
        let mut tree = ExtentTree::new();
        for i in 0..12u64 {
            tree.insert(mk_extent(i * 0x1000, 0x1000)).unwrap();
        }
        assert_eq!(tree.len(), 12);
        // Les 4 derniers sont dans le spill.
        assert_eq!(tree.spill.len(), 4);
    }

    #[test]
    fn test_total_data_size() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x1000)).unwrap();
        tree.insert(mk_extent(0x2000, 0x2000)).unwrap();
        assert_eq!(tree.total_data_size(), 0x3000);
    }

    #[test]
    fn test_sparse_size() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x1000)).unwrap(); // dense
        tree.insert(mk_sparse(0x2000, 0x1000)).unwrap(); // sparse
        assert_eq!(tree.sparse_size(), 0x1000);
        assert_eq!(tree.allocated_data_size(), 0x1000);
    }

    #[test]
    fn test_remove_at_offset() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x1000)).unwrap();
        tree.insert(mk_extent(0x1000, 0x1000)).unwrap();
        let removed = tree.remove_at_offset(0x500).unwrap();
        assert_eq!(removed.logical_offset, 0);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_validate_sorted() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0x2000, 0x1000)).unwrap();
        tree.insert(mk_extent(0, 0x1000)).unwrap(); // inséré avant
        tree.validate().unwrap(); // doit être trié
    }

    #[test]
    fn test_disk_roundtrip() {
        let mut tree = ExtentTree::new();
        tree.insert(mk_extent(0, 0x1000)).unwrap();
        tree.insert(mk_extent(0x2000, 0x1000)).unwrap();
        let disk = tree.to_disk_vec().unwrap();
        let tree2 = ExtentTree::from_disk_slice(&disk).unwrap();
        assert_eq!(tree2.len(), 2);
        assert_eq!(tree2.total_data_size(), tree.total_data_size());
    }
}
