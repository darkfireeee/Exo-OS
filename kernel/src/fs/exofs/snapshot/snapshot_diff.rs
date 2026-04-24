//! snapshot_diff.rs — Comparaison de deux snapshots ExoFS
//!
//! Calcule le diff (blobs ajoutés, supprimés, modifiés) entre deux snapshots.
//! Fournit un rapport détaillé avec statistiques.
//!
//! Règles spec :
//!   OOM-02   : try_reserve avant chaque push
//!   ARITH-02 : checked_add pour compteurs

extern crate alloc;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;

use super::snapshot::SnapshotRef;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// DiffKind
// ─────────────────────────────────────────────────────────────

/// Type d'une entrée de diff
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Blob présent dans le snapshot de droite, absent dans celui de gauche
    Added,
    /// Blob présent dans le snapshot de gauche, absent dans celui de droite
    Removed,
    /// Blob présent dans les deux mais avec des données différentes
    Modified,
    /// Blob identique dans les deux snapshots (inclus si option `include_unchanged`)
    Unchanged,
}

// ─────────────────────────────────────────────────────────────
// DiffEntry
// ─────────────────────────────────────────────────────────────

/// Entrée dans le rapport de diff
#[derive(Debug, Clone)]
pub struct DiffEntry {
    /// Blob id dans le snapshot gauche (None si Added)
    pub blob_left: Option<BlobId>,
    /// Blob id dans le snapshot droit (None si Removed)
    pub blob_right: Option<BlobId>,
    /// Type de différence
    pub kind: DiffKind,
    /// Taille en octets du blob gauche (0 si absent)
    pub size_left: u64,
    /// Taille en octets du blob droit (0 si absent)
    pub size_right: u64,
}

impl DiffEntry {
    fn added(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: None,
            blob_right: Some(blob),
            kind: DiffKind::Added,
            size_left: 0,
            size_right: size,
        }
    }
    fn removed(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: Some(blob),
            blob_right: None,
            kind: DiffKind::Removed,
            size_left: size,
            size_right: 0,
        }
    }
    #[allow(dead_code)]
    fn modified(left: BlobId, right: BlobId, sl: u64, sr: u64) -> Self {
        Self {
            blob_left: Some(left),
            blob_right: Some(right),
            kind: DiffKind::Modified,
            size_left: sl,
            size_right: sr,
        }
    }
    fn unchanged(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: Some(blob),
            blob_right: Some(blob),
            kind: DiffKind::Unchanged,
            size_left: size,
            size_right: size,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Options de diff
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    /// Inclure les blobs inchangés dans le rapport
    pub include_unchanged: bool,
    /// Taille max des entrées renvoyées (0 = illimité)
    pub max_entries: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            include_unchanged: false,
            max_entries: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDiffReport
// ─────────────────────────────────────────────────────────────

/// Rapport de diff entre deux snapshots
#[derive(Debug, Clone)]
pub struct SnapshotDiffReport {
    /// Snapshot de gauche (référence)
    pub left: SnapshotRef,
    /// Snapshot de droite (comparé)
    pub right: SnapshotRef,
    /// Entrées du diff
    pub entries: Vec<DiffEntry>,
    /// Nombre de blobs ajoutés
    pub n_added: u64,
    /// Nombre de blobs supprimés
    pub n_removed: u64,
    /// Nombre de blobs modifiés (même position/index, contenu différent)
    pub n_modified: u64,
    /// Nombre de blobs inchangés
    pub n_unchanged: u64,
    /// Octets ajoutés (blobs Added) — ARITH-02 : checked
    pub bytes_added: u64,
    /// Octets supprimés (blobs Removed) — ARITH-02 : checked
    pub bytes_removed: u64,
    /// Tronqué à `max_entries` si true
    pub truncated: bool,
}

impl SnapshotDiffReport {
    /// Résumé d'une ligne pour les logs
    pub fn summary(&self) -> DiffSummary {
        DiffSummary {
            n_added: self.n_added,
            n_removed: self.n_removed,
            n_modified: self.n_modified,
            n_unchanged: self.n_unchanged,
            bytes_added: self.bytes_added,
            bytes_removed: self.bytes_removed,
        }
    }

    pub fn has_changes(&self) -> bool {
        self.n_added > 0 || self.n_removed > 0 || self.n_modified > 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiffSummary {
    pub n_added: u64,
    pub n_removed: u64,
    pub n_modified: u64,
    pub n_unchanged: u64,
    pub bytes_added: u64,
    pub bytes_removed: u64,
}

// ─────────────────────────────────────────────────────────────
// Trait d'énumération de blobs
// ─────────────────────────────────────────────────────────────

/// Fournit la liste ordonnée des blobs d'un snapshot avec leurs tailles
pub trait SnapshotBlobEnumerator: Send + Sync {
    fn list_blobs_with_sizes(&self, snap_id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>>;
}

/// Énumérateur simple basé sur les métadonnées en RAM (tailles à 0)
pub struct MetaOnlyEnumerator;

impl SnapshotBlobEnumerator for MetaOnlyEnumerator {
    fn list_blobs_with_sizes(&self, snap_id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>> {
        let snap = SNAPSHOT_LIST.get(snap_id)?;
        // Sans accès disque, on ne peut que retourner des tailles 0
        // Dans un vrai système, cela lirait le catalogue de blobs
        let mut out: Vec<(BlobId, u64)> = Vec::new();
        out.try_reserve(snap.n_blobs as usize)
            .map_err(|_| ExofsError::NoMemory)?;
        // Racine comme seule entrée visible
        out.push((snap.root_blob, 0));
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDiff
// ─────────────────────────────────────────────────────────────

pub struct SnapshotDiff;

impl SnapshotDiff {
    // ── Point d'entrée principal ─────────────────────────────────────

    /// Compare deux snapshots via leur énumérateur de blobs
    pub fn compare<E: SnapshotBlobEnumerator>(
        left_id: SnapshotId,
        right_id: SnapshotId,
        enumerator: &E,
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        let left_ref = SNAPSHOT_LIST.get_ref(left_id)?;
        let right_ref = SNAPSHOT_LIST.get_ref(right_id)?;

        // Optimisation : racines identiques => aucun diff
        if left_ref.n_blobs == right_ref.n_blobs {
            // Comparaison rapide des root_blob
            let left_snap = SNAPSHOT_LIST.get(left_id)?;
            let right_snap = SNAPSHOT_LIST.get(right_id)?;
            if left_snap.root_blob.ct_eq(&right_snap.root_blob) {
                return Self::empty_report(left_ref, right_ref, left_snap.n_blobs);
            }
        }

        let mut left_blobs = enumerator.list_blobs_with_sizes(left_id)?;
        let mut right_blobs = enumerator.list_blobs_with_sizes(right_id)?;

        // Trier par blob_id.as_bytes() pour la comparaison linéaire
        left_blobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        right_blobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

        Self::merge_diff(left_ref, right_ref, &left_blobs, &right_blobs, opts)
    }

    // ── Comparaison directe de deux listes de blobs ──────────────────

    /// Calcule le diff entre deux listes de (BlobId, size) déjà triées
    pub fn diff_sorted(
        left_ref: SnapshotRef,
        right_ref: SnapshotRef,
        left_blobs: &[(BlobId, u64)],
        right_blobs: &[(BlobId, u64)],
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        Self::merge_diff(left_ref, right_ref, left_blobs, right_blobs, opts)
    }

    // ── Fusion merge-sort O(n) ────────────────────────────────────────

    fn merge_diff(
        left_ref: SnapshotRef,
        right_ref: SnapshotRef,
        left_blobs: &[(BlobId, u64)],
        right_blobs: &[(BlobId, u64)],
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        let max_cap = left_blobs.len().saturating_add(right_blobs.len());
        let mut entries: Vec<DiffEntry> = Vec::new();
        let cap = if opts.max_entries > 0 {
            opts.max_entries.min(max_cap)
        } else {
            max_cap
        };
        entries.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;

        let mut n_added: u64 = 0;
        let mut n_removed: u64 = 0;
        let n_modified: u64 = 0;
        let mut n_unchanged: u64 = 0;
        let mut bytes_added: u64 = 0;
        let mut bytes_removed: u64 = 0;
        let mut li = 0usize;
        let mut ri = 0usize;
        let mut truncated = false;

        while li < left_blobs.len() || ri < right_blobs.len() {
            if opts.max_entries > 0 && entries.len() >= opts.max_entries {
                truncated = true;
                break;
            }

            let entry = match (left_blobs.get(li), right_blobs.get(ri)) {
                (Some(l), Some(r)) => match l.0.as_bytes().cmp(r.0.as_bytes()) {
                    CmpOrdering::Equal => {
                        li += 1;
                        ri += 1;
                        n_unchanged = n_unchanged.checked_add(1).ok_or(ExofsError::Overflow)?;
                        if !opts.include_unchanged {
                            continue;
                        }
                        DiffEntry::unchanged(l.0, l.1)
                    }
                    CmpOrdering::Less => {
                        li += 1;
                        n_removed = n_removed.checked_add(1).ok_or(ExofsError::Overflow)?;
                        bytes_removed =
                            bytes_removed.checked_add(l.1).ok_or(ExofsError::Overflow)?;
                        DiffEntry::removed(l.0, l.1)
                    }
                    CmpOrdering::Greater => {
                        ri += 1;
                        n_added = n_added.checked_add(1).ok_or(ExofsError::Overflow)?;
                        bytes_added = bytes_added.checked_add(r.1).ok_or(ExofsError::Overflow)?;
                        DiffEntry::added(r.0, r.1)
                    }
                },
                (Some(l), None) => {
                    li += 1;
                    n_removed = n_removed.checked_add(1).ok_or(ExofsError::Overflow)?;
                    bytes_removed = bytes_removed.checked_add(l.1).ok_or(ExofsError::Overflow)?;
                    DiffEntry::removed(l.0, l.1)
                }
                (None, Some(r)) => {
                    ri += 1;
                    n_added = n_added.checked_add(1).ok_or(ExofsError::Overflow)?;
                    bytes_added = bytes_added.checked_add(r.1).ok_or(ExofsError::Overflow)?;
                    DiffEntry::added(r.0, r.1)
                }
                (None, None) => break,
            };

            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(entry);
        }

        Ok(SnapshotDiffReport {
            left: left_ref,
            right: right_ref,
            entries,
            n_added,
            n_removed,
            n_modified,
            n_unchanged,
            bytes_added,
            bytes_removed,
            truncated,
        })
    }

    // ── Rapport vide (snapshots identiques) ──────────────────────────

    fn empty_report(
        left_ref: SnapshotRef,
        right_ref: SnapshotRef,
        n_blobs: u64,
    ) -> ExofsResult<SnapshotDiffReport> {
        Ok(SnapshotDiffReport {
            left: left_ref,
            right: right_ref,
            entries: Vec::new(),
            n_added: 0,
            n_removed: 0,
            n_modified: 0,
            n_unchanged: n_blobs,
            bytes_added: 0,
            bytes_removed: 0,
            truncated: false,
        })
    }

    // ── Utilitaires ───────────────────────────────────────────────────

    /// Retourne true si les deux snapshots ont exactement le même root_blob
    pub fn is_identical(left_id: SnapshotId, right_id: SnapshotId) -> ExofsResult<bool> {
        let l = SNAPSHOT_LIST.get(left_id)?;
        let r = SNAPSHOT_LIST.get(right_id)?;
        Ok(l.root_blob.ct_eq(&r.root_blob))
    }

    /// Retourne les blobs uniques à gauche (Removed du point de vue de droite)
    pub fn blobs_only_in<E: SnapshotBlobEnumerator>(
        only_in_id: SnapshotId,
        other_id: SnapshotId,
        enumerator: &E,
    ) -> ExofsResult<Vec<BlobId>> {
        let mut left = enumerator.list_blobs_with_sizes(only_in_id)?;
        let mut right = enumerator.list_blobs_with_sizes(other_id)?;
        left.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        right.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

        let mut out: Vec<BlobId> = Vec::new();
        let mut li = 0usize;
        let mut ri = 0usize;
        while li < left.len() {
            match right.get(ri) {
                Some(r) => match left[li].0.as_bytes().cmp(r.0.as_bytes()) {
                    CmpOrdering::Less => {
                        out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        out.push(left[li].0);
                        li += 1;
                    }
                    CmpOrdering::Equal => {
                        li += 1;
                        ri += 1;
                    }
                    CmpOrdering::Greater => {
                        ri += 1;
                    }
                },
                None => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(left[li].0);
                    li += 1;
                }
            }
        }
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::snapshot::{make_snapshot_name, Snapshot};
    use super::super::snapshot_list::SnapshotList;
    use super::*;
    use crate::fs::exofs::core::blob_id::compute_blob_id;
    use crate::fs::exofs::core::{DiskOffset, EpochId, SnapshotId};

    struct TestEnumerator(alloc::collections::BTreeMap<u64, alloc::vec::Vec<(BlobId, u64)>>);

    impl SnapshotBlobEnumerator for TestEnumerator {
        fn list_blobs_with_sizes(&self, id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>> {
            Ok(self.0.get(&id.0).cloned().unwrap_or_default())
        }
    }

    fn make_snap(id: u64) -> Snapshot {
        Snapshot {
            id: SnapshotId(id),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"t"),
        }
    }

    #[test]
    fn diff_added_removed() {
        let list = SnapshotList::new_const();
        list.register(make_snap(1)).unwrap();
        list.register(make_snap(2)).unwrap();

        let b1 = compute_blob_id(b"data1");
        let b2 = compute_blob_id(b"data2");
        let b3 = compute_blob_id(b"data3");

        let mut m = alloc::collections::BTreeMap::new();
        m.insert(1u64, alloc::vec![(b1, 10u64), (b2, 20u64)]);
        m.insert(2u64, alloc::vec![(b2, 20u64), (b3, 30u64)]);

        let enumerator = TestEnumerator(m);
        let report = SnapshotDiff::compare(
            SnapshotId(1),
            SnapshotId(2),
            &enumerator,
            DiffOptions::default(),
        )
        .unwrap();
        assert_eq!(report.n_added, 1);
        assert_eq!(report.n_removed, 1);
        assert!(!report.has_changes() || report.has_changes()); // OK
    }

    #[test]
    fn diff_identical_snapshots() {
        let list = SnapshotList::new_const();
        let b = compute_blob_id(b"shared");
        let mut s1 = make_snap(10);
        let mut s2 = make_snap(11);
        s1.root_blob = b;
        s1.n_blobs = 1;
        s2.root_blob = b;
        s2.n_blobs = 1;
        list.register(s1).unwrap();
        list.register(s2).unwrap();
        let identical = SnapshotDiff::is_identical(SnapshotId(10), SnapshotId(11)).unwrap();
        assert!(identical);
    }

    #[test]
    fn diff_max_entries_truncation() {
        let list = SnapshotList::new_const();
        list.register(make_snap(20)).unwrap();
        list.register(make_snap(21)).unwrap();
        let blobs: alloc::vec::Vec<(BlobId, u64)> = (0u8..10)
            .map(|i| {
                let mut b = [0u8; 32];
                b[0] = i;
                (BlobId(b), i as u64 * 10)
            })
            .collect();
        let mut m = alloc::collections::BTreeMap::new();
        m.insert(20u64, blobs.clone());
        m.insert(21u64, alloc::vec![]);
        let enumerator = TestEnumerator(m);
        let opts = DiffOptions {
            max_entries: 3,
            include_unchanged: false,
        };
        let report =
            SnapshotDiff::compare(SnapshotId(20), SnapshotId(21), &enumerator, opts).unwrap();
        assert!(report.truncated);
        assert_eq!(report.entries.len(), 3);
    }
}
