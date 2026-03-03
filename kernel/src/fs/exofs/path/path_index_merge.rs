//! path_index_merge.rs — Fusion de deux PathIndex en un seul index.
//!
//! Opération inverse du split. Utilisée lors du vidage d un sous-répertoire ou
//! après une série de suppressions qui fait chuter le nombre d entrées en-dessous
//! du seuil de merge.
//!
//! ## Règles spec appliquées
//! - **ARITH-02** : `checked_add` sur les calculs de taille/count.
//! - **OOM-02** : `try_reserve(1)` avant les insertions.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use super::path_index::{PathIndex, PATH_INDEX_SPLIT_THRESHOLD};
use super::path_component::validate_component;

// ── MergeResult ───────────────────────────────────────────────────────────────

/// Résultat d une fusion.
pub struct MergeResult {
    pub merged:      PathIndex,
    pub total_count: usize,
    pub reclaimed:   u64,
}

// ── MergeMetrics ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MergeMetrics {
    pub total_before:       usize,
    pub total_after:        usize,
    pub duplicates_ignored: usize,
    pub compacted:          bool,
}

// ── PathIndexMerger ───────────────────────────────────────────────────────────

pub struct PathIndexMerger;

impl PathIndexMerger {
    /// Fusionne `low` et `high` en un nouvel index.
    pub fn merge(
        low:        &PathIndex,
        high:       &PathIndex,
        parent_oid: ObjectId,
    ) -> ExofsResult<MergeResult> {
        let total = low.len()
            .checked_add(high.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if total > PATH_INDEX_SPLIT_THRESHOLD as usize {
            return Err(ExofsError::NoSpace);
        }
        let mut merged = PathIndex::new(parent_oid);
        let mut duplicates = 0usize;

        for e in low.entries() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;
            match merged.insert(&comp, e.oid.clone(), e.kind) {
                Ok(()) => {}
                Err(ExofsError::ObjectAlreadyExists) => { duplicates = duplicates.saturating_add(1); }
                Err(err) => return Err(err),
            }
        }
        for e in high.entries() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;
            match merged.insert(&comp, e.oid.clone(), e.kind) {
                Ok(()) => {}
                Err(ExofsError::ObjectAlreadyExists) => { duplicates = duplicates.saturating_add(1); }
                Err(err) => return Err(err),
            }
        }

        merged.dirty = true;
        let total_count = merged.len();
        let reclaimed   = (duplicates as u64).saturating_mul(60);
        Ok(MergeResult { merged, total_count, reclaimed })
    }

    pub fn merge_with_metrics(
        low:        &PathIndex,
        high:       &PathIndex,
        parent_oid: ObjectId,
    ) -> ExofsResult<(MergeResult, MergeMetrics)> {
        let total_before = low.len().checked_add(high.len()).ok_or(ExofsError::OffsetOverflow)?;
        let result = Self::merge(low, high, parent_oid)?;
        let metrics = MergeMetrics {
            total_before,
            total_after:        result.total_count,
            duplicates_ignored: total_before.saturating_sub(result.total_count),
            compacted:          false,
        };
        Ok((result, metrics))
    }

    pub fn can_merge(low: &PathIndex, high: &PathIndex) -> bool {
        low.len()
            .checked_add(high.len())
            .map(|t| t <= PATH_INDEX_SPLIT_THRESHOLD as usize)
            .unwrap_or(false)
    }

    pub fn merge_many(
        indices:    &[&PathIndex],
        parent_oid: ObjectId,
    ) -> ExofsResult<MergeResult> {
        if indices.is_empty() {
            return Ok(MergeResult {
                merged:      PathIndex::new(parent_oid),
                total_count: 0,
                reclaimed:   0,
            });
        }
        if indices.len() == 1 {
            let mut merged = PathIndex::new(parent_oid);
            for e in indices[0].entries() {
                let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;
                merged.insert(&comp, e.oid.clone(), e.kind)?;
            }
            let total_count = merged.len();
            return Ok(MergeResult { merged, total_count, reclaimed: 0 });
        }
        let mut acc = PathIndex::new(parent_oid);
        let mut total_reclaimed: u64 = 0;
        for idx in indices {
            if acc.len().checked_add(idx.len()).map(|t| t > PATH_INDEX_SPLIT_THRESHOLD as usize).unwrap_or(true) {
                return Err(ExofsError::NoSpace);
            }
            for e in idx.entries() {
                let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;
                match acc.insert(&comp, e.oid.clone(), e.kind) {
                    Ok(()) => {}
                    Err(ExofsError::ObjectAlreadyExists) => { total_reclaimed = total_reclaimed.saturating_add(60); }
                    Err(err) => return Err(err),
                }
            }
        }
        let total_count = acc.len();
        Ok(MergeResult { merged: acc, total_count, reclaimed: total_reclaimed })
    }

    pub fn try_merge(
        low:        &PathIndex,
        high:       &PathIndex,
        parent_oid: ObjectId,
    ) -> Result<MergeResult, MergeRefusal> {
        if low.is_empty() && high.is_empty() { return Err(MergeRefusal::BothEmpty); }
        if !Self::can_merge(low, high) {
            return Err(MergeRefusal::TooBig {
                low_count:  low.len(),
                high_count: high.len(),
                threshold:  PATH_INDEX_SPLIT_THRESHOLD as usize,
            });
        }
        Self::merge(low, high, parent_oid).map_err(MergeRefusal::Error)
    }
}

// ── MergeRefusal ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum MergeRefusal {
    BothEmpty,
    TooBig { low_count: usize, high_count: usize, threshold: usize },
    Error(ExofsError),
}

// ── MergeConflictPolicy ───────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeConflictPolicy { PreferLow, PreferHigh, Reject }

impl Default for MergeConflictPolicy { fn default() -> Self { Self::PreferLow } }

// ── merge_with_policy ─────────────────────────────────────────────────────────

pub fn merge_with_policy(
    low:        &PathIndex,
    high:       &PathIndex,
    parent_oid: ObjectId,
    policy:     MergeConflictPolicy,
) -> ExofsResult<MergeResult> {
    let total = low.len().checked_add(high.len()).ok_or(ExofsError::OffsetOverflow)?;
    if total > PATH_INDEX_SPLIT_THRESHOLD as usize { return Err(ExofsError::NoSpace); }
    let mut merged = PathIndex::new(parent_oid);
    let (first, second): (&PathIndex, &PathIndex) = match policy {
        MergeConflictPolicy::PreferLow  => (low, high),
        MergeConflictPolicy::PreferHigh => (high, low),
        MergeConflictPolicy::Reject     => (low, high),
    };
    let mut dups = 0usize;
    for e in first.entries() {
        let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;
        merged.insert(&comp, e.oid.clone(), e.kind)?;
    }
    for e in second.entries() {
        let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;
        match merged.insert(&comp, e.oid.clone(), e.kind) {
            Ok(()) => {}
            Err(ExofsError::ObjectAlreadyExists) => {
                if policy == MergeConflictPolicy::Reject { return Err(ExofsError::ObjectAlreadyExists); }
                dups = dups.saturating_add(1);
            }
            Err(err) => return Err(err),
        }
    }
    merged.dirty = true;
    let total_count = merged.len();
    let reclaimed   = (dups as u64).saturating_mul(60);
    Ok(MergeResult { merged, total_count, reclaimed })
}

// ── MergeRequest ──────────────────────────────────────────────────────────────

pub struct MergeRequest {
    pub parent_oid: ObjectId,
    pub policy:     MergeConflictPolicy,
    pub verify:     bool,
}

impl MergeRequest {
    pub fn new(parent_oid: ObjectId) -> Self {
        MergeRequest { parent_oid, policy: MergeConflictPolicy::PreferLow, verify: true }
    }

    pub fn with_policy(mut self, p: MergeConflictPolicy) -> Self { self.policy = p; self }

    pub fn execute(self, low: &PathIndex, high: &PathIndex) -> ExofsResult<(MergeResult, MergeMetrics)> {
        let total_before = low.len().checked_add(high.len()).ok_or(ExofsError::OffsetOverflow)?;
        let result = merge_with_policy(low, high, self.parent_oid, self.policy)?;
        if self.verify && result.total_count > PATH_INDEX_SPLIT_THRESHOLD as usize {
            return Err(ExofsError::InternalError);
        }
        let metrics = MergeMetrics {
            total_before,
            total_after:        result.total_count,
            duplicates_ignored: total_before.saturating_sub(result.total_count),
            compacted:          false,
        };
        Ok((result, metrics))
    }
}

// ── MergeValidator ───────────────────────────────────────────────────────────

/// Validateur post-merge — vérifie l intégrité du résultat.
pub struct MergeValidator;

impl MergeValidator {
    /// Vérifie que le résultat contient bien les entrées attendues.
    ///
    /// - Aucune entrée en double.
    /// - Toutes les entrées de `low` et `high` sont présentes (sauf doublons).
    pub fn verify(
        result:    &MergeResult,
        low:       &PathIndex,
        high:      &PathIndex,
    ) -> ExofsResult<()> {
        // Vérifier count cohérent.
        let max_expected = low.len()
            .checked_add(high.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if result.total_count > max_expected {
            return Err(ExofsError::InternalError);
        }
        // Vérifier que toutes les entrées de low sont dans merged.
        for e in low.entries() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;
            if result.merged.lookup(&comp).is_none() {
                return Err(ExofsError::InternalError);
            }
        }
        // Vérifier que toutes les entrées de high sont dans merged.
        for e in high.entries() {
            let comp = validate_component(e.name_bytes())
                .map_err(|_| ExofsError::InternalError)?;
            if result.merged.lookup(&comp).is_none() {
                return Err(ExofsError::InternalError);
            }
        }
        Ok(())
    }

    /// Vérifie qu aucune entrée en double n existe dans l index fusionné.
    pub fn verify_no_duplicates(merged: &PathIndex) -> ExofsResult<()> {
        // Collecter tous les hashes.
        let mut hashes: Vec<u64> = Vec::new();
        for e in merged.entries() {
            hashes.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            hashes.push(e.hash);
        }
        hashes.sort_unstable();
        // Vérifier l unicité.
        for w in hashes.windows(2) {
            if w[0] == w[1] {
                return Err(ExofsError::InternalError);
            }
        }
        Ok(())
    }
}

// ── MergeThresholdAdvisor ────────────────────────────────────────────────────

/// Conseiller sur le seuil de merge.
///
/// Recommande si une fusion est opportune en tenant compte de :
/// - L occupation des deux index.
/// - Le seuil de split configuré.
/// - Un facteur d hysteresis pour éviter les boucles split/merge.
pub struct MergeThresholdAdvisor {
    /// Facteur d hysteresis : fusion conseillée si count < threshold * factor.
    pub hysteresis_factor: u32,
}

impl MergeThresholdAdvisor {
    pub fn new() -> Self {
        MergeThresholdAdvisor { hysteresis_factor: 50 } // 50 % du seuil
    }

    /// `true` si la fusion est conseillée.
    pub fn should_merge(&self, low: &PathIndex, high: &PathIndex) -> bool {
        let total = match low.len().checked_add(high.len()) {
            Some(t) => t,
            None    => return false,
        };
        let threshold = (PATH_INDEX_SPLIT_THRESHOLD as usize)
            .saturating_mul(self.hysteresis_factor as usize)
            / 100;
        total <= threshold && PathIndexMerger::can_merge(low, high)
    }

    /// Retourne le pourcentage d occupation combiné.
    pub fn combined_load_pct(&self, low: &PathIndex, high: &PathIndex) -> u32 {
        let total = low.len().saturating_add(high.len());
        (total as u32).saturating_mul(100) / PATH_INDEX_SPLIT_THRESHOLD.max(1)
    }
}

impl Default for MergeThresholdAdvisor {
    fn default() -> Self { Self::new() }
}

// ── Utilitaires ───────────────────────────────────────────────────────────────

/// Raccourci vers [`PathIndexMerger::merge`].
pub fn simple_merge(low: &PathIndex, high: &PathIndex, parent_oid: ObjectId) -> ExofsResult<MergeResult> {
    PathIndexMerger::merge(low, high, parent_oid)
}

/// `true` si les deux index méritent d être fusionnés (seuils + capacité).
pub fn should_merge(low: &PathIndex, high: &PathIndex) -> bool {
    low.needs_merge() && high.needs_merge() && PathIndexMerger::can_merge(low, high)
}

/// Fusionne et vérifie le résultat (mode sécurisé).
pub fn safe_merge(
    low:        &PathIndex,
    high:       &PathIndex,
    parent_oid: ObjectId,
) -> ExofsResult<MergeResult> {
    let result = PathIndexMerger::merge(low, high, parent_oid)?;
    MergeValidator::verify(&result, low, high)?;
    MergeValidator::verify_no_duplicates(&result.merged)?;
    Ok(result)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::path_component::validate_component;

    fn fake_oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    fn make_index(prefix: u8, n: usize) -> PathIndex {
        let mut idx = PathIndex::new(fake_oid(0));
        for i in 0..n {
            let name = alloc::format!("{}{:03}", prefix as char, i).into_bytes();
            idx.insert(&validate_component(&name).unwrap(), fake_oid(i as u8), 0).unwrap();
        }
        idx
    }

    #[test] fn test_merge_basic() {
        let low  = make_index(b'a', 5);
        let high = make_index(b'b', 5);
        let res  = PathIndexMerger::merge(&low, &high, fake_oid(0)).unwrap();
        assert_eq!(res.total_count, 10);
    }
    #[test] fn test_merge_too_large() {
        let low  = make_index(b'a', 100);
        let high = make_index(b'b', 100);
        assert!(matches!(PathIndexMerger::merge(&low, &high, fake_oid(0)), Err(ExofsError::NoSpace)));
    }
    #[test] fn test_can_merge() {
        let low  = make_index(b'c', 5);
        let high = make_index(b'd', 5);
        assert!(PathIndexMerger::can_merge(&low, &high));
    }
    #[test] fn test_with_metrics() {
        let low  = make_index(b'e', 3);
        let high = make_index(b'f', 3);
        let (res, m) = PathIndexMerger::merge_with_metrics(&low, &high, fake_oid(0)).unwrap();
        assert_eq!(res.total_count, 6);
        assert_eq!(m.total_before, 6);
    }
    #[test] fn test_duplicate_handling() {
        let mut low  = PathIndex::new(fake_oid(0));
        let mut high = PathIndex::new(fake_oid(0));
        let c = validate_component(b"shared").unwrap();
        low.insert(&c, fake_oid(1), 0).unwrap();
        high.insert(&c, fake_oid(2), 0).unwrap();
        let res = PathIndexMerger::merge(&low, &high, fake_oid(0)).unwrap();
        assert_eq!(res.total_count, 1);
        assert!(res.reclaimed > 0);
    }
    #[test] fn test_should_merge() {
        let low  = make_index(b'g', 2);
        let high = make_index(b'h', 2);
        assert!(should_merge(&low, &high));
    }
    #[test] fn test_merge_many() {
        let a = make_index(b'i', 3);
        let b = make_index(b'j', 3);
        let c = make_index(b'k', 3);
        let res = PathIndexMerger::merge_many(&[&a, &b, &c], fake_oid(0)).unwrap();
        assert_eq!(res.total_count, 9);
    }
    #[test] fn test_merge_empty() {
        let idx   = make_index(b'l', 3);
        let empty = PathIndex::new(fake_oid(0));
        let res = PathIndexMerger::merge(&idx, &empty, fake_oid(0)).unwrap();
        assert_eq!(res.total_count, 3);
    }
    #[test] fn test_conflict_reject() {
        let mut low  = PathIndex::new(fake_oid(0));
        let mut high = PathIndex::new(fake_oid(0));
        let c = validate_component(b"conflict").unwrap();
        low.insert(&c, fake_oid(1), 0).unwrap();
        high.insert(&c, fake_oid(2), 0).unwrap();
        assert!(matches!(
            merge_with_policy(&low, &high, fake_oid(0), MergeConflictPolicy::Reject),
            Err(ExofsError::ObjectAlreadyExists)
        ));
    }
}
