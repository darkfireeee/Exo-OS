//! path_index_split.rs — Division d un PathIndex en deux index fils.
//!
//! Lorsqu un [`PathIndex`] dépasse son seuil de split (`needs_split() == true`),
//! ce module le divise en un index "bas" (entrées hash < pivot) et un index "haut"
//! (entrées hash >= pivot).
//!
//! ## Stratégie
//! - Tri des entrées par hash.
//! - Pivot = médiane des hashes.
//! - Index bas  : entrées [0..mid].
//! - Index haut : entrées [mid..].
//!
//! ## Règles spec appliquées
//! - **ARITH-02** : `checked_add` sur les calculs de taille.
//! - **OOM-02** : `try_reserve(1)` avant les insertions.


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use super::path_index::{PathIndex, InMemoryEntry};
use super::path_component::validate_component;

// ── SplitResult ───────────────────────────────────────────────────────────────

/// Résultat d un split.
pub struct SplitResult {
    /// Index fils "bas" (hashes inférieurs au pivot).
    pub low:         PathIndex,
    /// Index fils "haut" (hashes supérieurs ou égaux au pivot).
    pub high:        PathIndex,
    /// Hash pivot utilisé pour la division.
    pub pivot_hash:  u64,
    /// Nombre d entrées transférées dans l index bas.
    pub low_count:   usize,
    /// Nombre d entrées transférées dans l index haut.
    pub high_count:  usize,
}

// ── PathIndexSplitter ─────────────────────────────────────────────────────────

/// Diviseur de PathIndex.
///
/// Consomme l index source et produit un [`SplitResult`].
pub struct PathIndexSplitter;

impl PathIndexSplitter {
    /// Divise `src` en deux index fils équilibrés.
    ///
    /// L index source est consommé (ses entrées sont drainées).
    /// Les deux index fils partagent le même `parent_oid`.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidArgument`]  si l index source a < 2 entrées.
    /// - [`ExofsError::NoMemory`]         si allocation impossible.
    /// - [`ExofsError::InternalError`]    si invariant interne violé.
    pub fn split(src: &mut PathIndex, parent_oid: ObjectId) -> ExofsResult<SplitResult> {
        if src.len() < 2 {
            return Err(ExofsError::InvalidArgument);
        }

        // ── 1. Collecte et tri par hash ───────────────────────────────────────
        let mut sorted: Vec<InMemoryEntry> = Vec::new();
        for e in src.entries() {
            sorted.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            sorted.push(e.clone());
        }
        sorted.sort_unstable_by_key(|e| e.hash);

        // ── 2. Calcul du pivot (médiane) ──────────────────────────────────────
        let mid = sorted.len() / 2;
        let pivot_hash = sorted[mid].hash;

        // ── 3. Création des deux index fils ───────────────────────────────────
        let mut low  = PathIndex::new(parent_oid.clone());
        let mut high = PathIndex::new(parent_oid);

        for (i, e) in sorted.iter().enumerate() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;

            if i < mid {
                low.insert(&comp, e.oid.clone(), e.kind)?;
            } else {
                high.insert(&comp, e.oid.clone(), e.kind)?;
            }
        }

        low.dirty  = true;
        high.dirty = true;

        let low_count  = low.len();
        let high_count = high.len();

        Ok(SplitResult { low, high, pivot_hash, low_count, high_count })
    }

    /// Divise en appliquant un pivot hash fourni explicitement.
    ///
    /// Utile quand le pivot doit être cohérent avec un arbre B+ existant.
    ///
    /// # Errors
    /// Mêmes que [`split`].
    pub fn split_at_hash(
        src: &mut PathIndex,
        parent_oid: ObjectId,
        pivot_hash: u64,
    ) -> ExofsResult<SplitResult> {
        if src.len() < 2 {
            return Err(ExofsError::InvalidArgument);
        }

        let mut low  = PathIndex::new(parent_oid.clone());
        let mut high = PathIndex::new(parent_oid);

        for e in src.entries() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;
            if e.hash < pivot_hash {
                low.insert(&comp, e.oid.clone(), e.kind)?;
            } else {
                high.insert(&comp, e.oid.clone(), e.kind)?;
            }
        }

        if low.is_empty() || high.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }

        low.dirty  = true;
        high.dirty = true;

        let low_count  = low.len();
        let high_count = high.len();

        Ok(SplitResult { low, high, pivot_hash, low_count, high_count })
    }

    /// Vérifie qu un split est nécessaire.
    #[inline]
    pub fn is_needed(idx: &PathIndex) -> bool {
        idx.needs_split()
    }

    /// Calcule le pivot optimal (hash médian) sans modifier l index.
    ///
    /// Retourne `None` si l index a moins de 2 entrées.
    pub fn optimal_pivot(idx: &PathIndex) -> Option<u64> {
        if idx.len() < 2 { return None; }
        let mut hashes: Vec<u64> = Vec::new();
        for e in idx.entries() {
            hashes.try_reserve(1).ok()?;
            hashes.push(e.hash);
        }
        hashes.sort_unstable();
        let mid = hashes.len() / 2;
        Some(hashes[mid])
    }

    /// Estimation de la taille sérialisée d un index après split.
    ///
    /// Permet de vérifier l espace disponible avant d effectuer le split.
    ///
    /// # ARITH-02
    pub fn estimate_split_size(src: &PathIndex) -> ExofsResult<(usize, usize)> {
        let half = src.len() / 2;
        // Taille header + entrées moyennes (44 + ~16 octets de nom).
        let avg_entry: usize = 44_usize.checked_add(16).ok_or(ExofsError::OffsetOverflow)?;
        let half_entries = half.checked_mul(avg_entry).ok_or(ExofsError::OffsetOverflow)?;
        let low_size  = (148_usize).checked_add(half_entries).ok_or(ExofsError::OffsetOverflow)?;
        let rem = src.len().saturating_sub(half);
        let rem_entries = rem.checked_mul(avg_entry).ok_or(ExofsError::OffsetOverflow)?;
        let high_size = (148_usize).checked_add(rem_entries).ok_or(ExofsError::OffsetOverflow)?;
        Ok((low_size, high_size))
    }

    /// Valide la cohérence d un SplitResult :
    /// - low  : tous les hashes < pivot.
    /// - high : tous les hashes >= pivot.
    /// - low.len() + high.len() == original_count.
    pub fn verify_split(
        result: &SplitResult,
        original_count: usize,
    ) -> ExofsResult<()> {
        // Vérifier cohérence count.
        let total = result.low_count
            .checked_add(result.high_count)
            .ok_or(ExofsError::OffsetOverflow)?;
        if total != original_count {
            return Err(ExofsError::InternalError);
        }

        // Vérifier hashes low < pivot.
        for e in result.low.entries() {
            if e.hash >= result.pivot_hash {
                return Err(ExofsError::InternalError);
            }
        }
        // Vérifier hashes high >= pivot.
        for e in result.high.entries() {
            if e.hash < result.pivot_hash {
                return Err(ExofsError::InternalError);
            }
        }
        Ok(())
    }
}

// ── SplitMetrics ──────────────────────────────────────────────────────────────

/// Métriques collectées lors d un split.
#[derive(Debug, Clone)]
pub struct SplitMetrics {
    /// Nombre total d entrées dans la source avant split.
    pub total_before:      usize,
    /// Nombre d entrées dans l index bas.
    pub low_count:         usize,
    /// Nombre d entrées dans l index haut.
    pub high_count:        usize,
    /// Ratio de déséquilibre (0.0 = parfaitement équilibré, 1.0 = tout d un côté).
    pub imbalance_pct:     u32,
    /// Hash pivot finalement utilisé.
    pub used_pivot:        u64,
    /// Seuil de split de la source.
    pub source_threshold:  u32,
}

impl SplitMetrics {
    /// Construit les métriques depuis un SplitResult.
    pub fn from_result(result: &SplitResult, total_before: usize, source_threshold: u32) -> Self {
        let low  = result.low_count as i64;
        let high = result.high_count as i64;
        let total = (total_before as i64).max(1);
        let diff  = (low - high).unsigned_abs() as u32;
        let imbalance_pct = (diff.saturating_mul(100)) / (total as u32).max(1);
        SplitMetrics {
            total_before,
            low_count:        result.low_count,
            high_count:       result.high_count,
            imbalance_pct,
            used_pivot:       result.pivot_hash,
            source_threshold,
        }
    }

    /// `true` si le split est fortement déséquilibré (>75 % d un côté).
    pub fn is_imbalanced(&self) -> bool { self.imbalance_pct > 75 }
}

// ── SplitPolicy ───────────────────────────────────────────────────────────────

/// Politique de split : comment choisir le pivot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitPolicy {
    /// Pivot médian (équilibre maximal).
    Median,
    /// Pivot au 25e percentile (favorise l index haut).
    Percentile25,
    /// Pivot au 75e percentile (favorise l index bas).
    Percentile75,
    /// Pivot fourni explicitement.
    Explicit(u64),
}

impl Default for SplitPolicy {
    fn default() -> Self { SplitPolicy::Median }
}

impl SplitPolicy {
    /// Calcule le hash pivot selon la politique donnée.
    ///
    /// # OOM-02
    pub fn compute_pivot(&self, src: &PathIndex) -> ExofsResult<u64> {
        match self {
            SplitPolicy::Explicit(h) => Ok(*h),
            policy => {
                let mut hashes: Vec<u64> = Vec::new();
                for e in src.entries() {
                    hashes.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    hashes.push(e.hash);
                }
                if hashes.is_empty() { return Err(ExofsError::InvalidArgument); }
                hashes.sort_unstable();
                let n = hashes.len();
                let idx = match policy {
                    SplitPolicy::Median       => n / 2,
                    SplitPolicy::Percentile25 => n / 4,
                    SplitPolicy::Percentile75 => (n * 3) / 4,
                    SplitPolicy::Explicit(_)  => unreachable!(),
                };
                Ok(hashes[idx.min(n - 1)])
            }
        }
    }
}

// ── SplitRequest ──────────────────────────────────────────────────────────────

/// Requête de split avec politique configurable.
pub struct SplitRequest {
    pub parent_oid: ObjectId,
    pub policy:     SplitPolicy,
    /// Si `true`, vérifie automatiquement le résultat après split.
    pub verify:     bool,
}

impl SplitRequest {
    /// Crée une requête de split avec politique médiane par défaut.
    pub fn new(parent_oid: ObjectId) -> Self {
        SplitRequest {
            parent_oid,
            policy: SplitPolicy::Median,
            verify: true,
        }
    }

    /// Modifie la politique.
    pub fn with_policy(mut self, p: SplitPolicy) -> Self {
        self.policy = p; self
    }

    /// Exécute le split.
    pub fn execute(self, src: &mut PathIndex) -> ExofsResult<(SplitResult, SplitMetrics)> {
        let total_before = src.len();
        let source_thr   = src.split_threshold;
        let pivot = self.policy.compute_pivot(src)?;
        let result = PathIndexSplitter::split_at_hash(src, self.parent_oid, pivot)?;
        if self.verify {
            PathIndexSplitter::verify_split(&result, total_before)?;
        }
        let metrics = SplitMetrics::from_result(&result, total_before, source_thr);
        Ok((result, metrics))
    }
}

// ── Fonctions utilitaires ─────────────────────────────────────────────────────

/// Divise un index en utilisant un split automatique (pivot médian).
///
/// Raccourci vers [`PathIndexSplitter::split`].
pub fn auto_split(src: &mut PathIndex, parent_oid: ObjectId) -> ExofsResult<SplitResult> {
    PathIndexSplitter::split(src, parent_oid)
}

/// Retourne `true` si l index doit être splitté selon le seuil par défaut.
pub fn should_split(idx: &PathIndex) -> bool {
    idx.needs_split()
}

/// Divise en collectant aussi les métriques (utile pour le monitoring).
pub fn split_with_metrics(
    src: &mut PathIndex,
    parent_oid: ObjectId,
    policy: SplitPolicy,
) -> ExofsResult<(SplitResult, SplitMetrics)> {
    SplitRequest::new(parent_oid)
        .with_policy(policy)
        .execute(src)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::path_component::validate_component;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32]; a[0] = b; ObjectId(a)
    }

    fn make_index(n: usize) -> PathIndex {
        let mut idx = PathIndex::new(fake_oid(0));
        idx.split_threshold = n as u32;
        for i in 0..n {
            let name: Vec<u8> = format!("entry{:04}", i).into_bytes();
            let c = validate_component(&name).unwrap();
            idx.insert(&c, fake_oid(i as u8), 0).unwrap();
        }
        idx
    }

    #[test] fn test_split_basic() {
        let mut idx = make_index(10);
        let res = PathIndexSplitter::split(&mut idx, fake_oid(0)).unwrap();
        assert_eq!(res.low_count + res.high_count, 10);
        assert!(res.low_count > 0);
        assert!(res.high_count > 0);
    }

    #[test] fn test_split_too_small() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"only").unwrap();
        idx.insert(&c, fake_oid(1), 0).unwrap();
        assert!(matches!(
            PathIndexSplitter::split(&mut idx, fake_oid(0)),
            Err(ExofsError::InvalidArgument)
        ));
    }

    #[test] fn test_verify_split() {
        let mut idx = make_index(8);
        let original = idx.len();
        let res = PathIndexSplitter::split(&mut idx, fake_oid(0)).unwrap();
        PathIndexSplitter::verify_split(&res, original).unwrap();
    }

    #[test] fn test_optimal_pivot() {
        let idx = make_index(10);
        let p = PathIndexSplitter::optimal_pivot(&idx);
        assert!(p.is_some());
    }

    #[test] fn test_split_at_hash() {
        let mut idx = make_index(10);
        let pivot = PathIndexSplitter::optimal_pivot(&idx).unwrap();
        let original = idx.len();
        let res = PathIndexSplitter::split_at_hash(&mut idx, fake_oid(0), pivot).unwrap();
        assert_eq!(res.low_count + res.high_count, original);
    }

    #[test] fn test_estimate_size() {
        let idx = make_index(10);
        let (low, high) = PathIndexSplitter::estimate_split_size(&idx).unwrap();
        assert!(low > 148);
        assert!(high > 148);
    }
}
