//! snapshot_delete.rs — Suppression sécurisée d'un snapshot ExoFS
//!
//! Vérifie les conditions de suppression (protection, enfants actifs,
//! montage en cours) avant de retirer le snapshot du registre.
//!
//! Règles spec :
//!   OOM-02   : try_reserve avant chaque Vec::push
//!   ARITH-02 : checked_add pour compteurs


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};
use super::snapshot::Snapshot;
use super::snapshot_list::SNAPSHOT_LIST;

// ─────────────────────────────────────────────────────────────
// Options de suppression
// ─────────────────────────────────────────────────────────────

/// Options transmises à `SnapshotDeleter::delete`
#[derive(Debug, Clone, Copy)]
pub struct DeleteOptions {
    /// Supprime récursivement les enfants avant le parent
    pub cascade: bool,
    /// Force la suppression même si le snapshot est protégé
    pub force:   bool,
    /// Ne supprime pas si le snapshot est monté
    pub skip_mounted: bool,
}

impl Default for DeleteOptions {
    fn default() -> Self {
        Self { cascade: false, force: false, skip_mounted: true }
    }
}

// ─────────────────────────────────────────────────────────────
// Résultat
// ─────────────────────────────────────────────────────────────

/// Résultat d'une suppression
#[derive(Debug, Clone)]
pub struct DeleteResult {
    /// Identifiant supprimé
    pub id: SnapshotId,
    /// Octets libérés
    pub freed_bytes: u64,
    /// Nombre de snapshots enfants également supprimés (cascade)
    pub n_cascade: u32,
    /// Identifiants des enfants supprimés (cascade)
    pub cascade_ids: Vec<SnapshotId>,
}

// ─────────────────────────────────────────────────────────────
// Raisons de rejet
// ─────────────────────────────────────────────────────────────

/// Raison pour laquelle la suppression est impossible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteDenyReason {
    /// Snapshot protégé contre la suppression
    Protected,
    /// Des enfants existent (sans cascade activée)
    HasChildren,
    /// Snapshot actuellement monté
    Mounted,
    /// Snapshot en cours de restauration
    Restoring,
    /// Snapshot en cours de streaming
    Streaming,
    /// Snapshot introuvable
    NotFound,
}

impl From<DeleteDenyReason> for ExofsError {
    fn from(r: DeleteDenyReason) -> Self {
        match r {
            DeleteDenyReason::Protected   => ExofsError::InvalidState,
            DeleteDenyReason::HasChildren => ExofsError::InvalidState,
            DeleteDenyReason::Mounted     => ExofsError::InvalidState,
            DeleteDenyReason::Restoring   => ExofsError::InvalidState,
            DeleteDenyReason::Streaming   => ExofsError::InvalidState,
            DeleteDenyReason::NotFound    => ExofsError::NotFound,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDeleter
// ─────────────────────────────────────────────────────────────

pub struct SnapshotDeleter;

impl SnapshotDeleter {
    // ── Point d'entrée principal ─────────────────────────────────────

    /// Supprime un snapshot après validation
    pub fn delete(id: SnapshotId, opts: DeleteOptions) -> ExofsResult<DeleteResult> {
        // Charger le snapshot pour validation
        let snap = SNAPSHOT_LIST.get(id).map_err(|_| ExofsError::NotFound)?;

        // Vérifier les pré-conditions
        Self::check_preconditions(&snap, opts)?;

        // Gérer la suppression en cascade
        let mut total_freed: u64 = 0;
        let mut cascade_ids: Vec<SnapshotId> = Vec::new();
        let mut n_cascade: u32 = 0;

        if opts.cascade {
            Self::cascade_delete(id, opts, &mut total_freed, &mut cascade_ids, &mut n_cascade)?;
        } else {
            // Vérification qu'aucun enfant n'existe
            let children = SNAPSHOT_LIST.children_of(id)?;
            if !children.is_empty() {
                return Err(ExofsError::from(DeleteDenyReason::HasChildren));
            }
        }

        // Supprimer le snapshot principal
        let removed = SNAPSHOT_LIST.remove(id)?;
        total_freed = total_freed.saturating_add(removed.total_bytes);

        Ok(DeleteResult {
            id,
            freed_bytes: total_freed,
            n_cascade,
            cascade_ids,
        })
    }

    // ── Suppressions multiples ───────────────────────────────────────

    /// Supprime plusieurs snapshots (sans cascade individuelle)
    ///
    /// OOM-02 : try_reserve avant push
    pub fn delete_batch(ids: &[SnapshotId], opts: DeleteOptions) -> ExofsResult<Vec<DeleteResult>> {
        let mut results: Vec<DeleteResult> = Vec::new();
        results.try_reserve(ids.len()).map_err(|_| ExofsError::NoMemory)?;
        for &id in ids {
            let r = Self::delete(id, opts)?;
            results.push(r);
        }
        Ok(results)
    }

    // ── Vérification pré-conditions ──────────────────────────────────

    pub fn check_preconditions(snap: &Snapshot, opts: DeleteOptions) -> ExofsResult<()> {
        if snap.is_protected() && !opts.force {
            return Err(ExofsError::from(DeleteDenyReason::Protected));
        }
        if snap.is_mounted() && opts.skip_mounted {
            return Err(ExofsError::from(DeleteDenyReason::Mounted));
        }
        if snap.is_restoring() {
            return Err(ExofsError::from(DeleteDenyReason::Restoring));
        }
        if snap.is_streaming() {
            return Err(ExofsError::from(DeleteDenyReason::Streaming));
        }
        Ok(())
    }

    // ── Suppression récursive ────────────────────────────────────────

    /// Supprime récursivement les enfants (pré-ordre enfants avant parent)
    fn cascade_delete(
        parent_id: SnapshotId,
        opts: DeleteOptions,
        total_freed: &mut u64,
        cascade_ids: &mut Vec<SnapshotId>,
        n_cascade: &mut u32,
    ) -> ExofsResult<()> {
        let children = SNAPSHOT_LIST.children_of(parent_id)?;
        for child_id in children {
            // Récursion sur les petits-enfants
            Self::cascade_delete(child_id, opts, total_freed, cascade_ids, n_cascade)?;

            let child = SNAPSHOT_LIST.get(child_id).map_err(|_| ExofsError::NotFound)?;
            // Vérification force pour les enfants aussi
            if child.is_protected() && !opts.force {
                return Err(ExofsError::from(DeleteDenyReason::Protected));
            }
            let removed = SNAPSHOT_LIST.remove(child_id)?;
            *total_freed = total_freed.saturating_add(removed.total_bytes);

            // OOM-02 : try_reserve avant push
            cascade_ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            cascade_ids.push(child_id);
            *n_cascade = n_cascade.checked_add(1).ok_or(ExofsError::Overflow)?;
        }
        Ok(())
    }

    // ── Utilitaires de diagnostic ────────────────────────────────────

    /// Retourne true si le snapshot est éligible à la suppression
    pub fn can_delete(id: SnapshotId, opts: DeleteOptions) -> bool {
        let Ok(snap) = SNAPSHOT_LIST.get(id) else { return false };
        if Self::check_preconditions(&snap, opts).is_err() { return false; }
        // Sans cascade : vérifier l'absence d'enfants
        if !opts.cascade {
            let Ok(children) = SNAPSHOT_LIST.children_of(id) else { return false };
            if !children.is_empty() { return false; }
        }
        true
    }

    /// Retourne la raison du rejet (ou None si suppression possible)
    pub fn deny_reason(id: SnapshotId, opts: DeleteOptions) -> Option<DeleteDenyReason> {
        let Ok(snap) = SNAPSHOT_LIST.get(id) else { return Some(DeleteDenyReason::NotFound) };
        if snap.is_protected() && !opts.force { return Some(DeleteDenyReason::Protected); }
        if snap.is_mounted() && opts.skip_mounted { return Some(DeleteDenyReason::Mounted); }
        if snap.is_restoring() { return Some(DeleteDenyReason::Restoring); }
        if snap.is_streaming() { return Some(DeleteDenyReason::Streaming); }
        if !opts.cascade {
            let Ok(children) = SNAPSHOT_LIST.children_of(id) else { return Some(DeleteDenyReason::NotFound) };
            if !children.is_empty() { return Some(DeleteDenyReason::HasChildren); }
        }
        None
    }

    /// Calcule récursivement les octets totaux qui seraient libérés (dry-run)
    pub fn estimate_cascade_freed(id: SnapshotId) -> ExofsResult<u64> {
        let snap = SNAPSHOT_LIST.get(id).map_err(|_| ExofsError::NotFound)?;
        let mut total = snap.total_bytes;
        let children = SNAPSHOT_LIST.children_of(id)?;
        for child_id in children {
            let child_freed = Self::estimate_cascade_freed(child_id)?;
            total = total.checked_add(child_freed).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }

    /// Retourne la liste ordonnée de suppression (enfants avant parents — dry-run)
    pub fn deletion_order(id: SnapshotId) -> ExofsResult<Vec<SnapshotId>> {
        let mut order: Vec<SnapshotId> = Vec::new();
        Self::collect_order(id, &mut order)?;
        Ok(order)
    }

    fn collect_order(id: SnapshotId, order: &mut Vec<SnapshotId>) -> ExofsResult<()> {
        let children = SNAPSHOT_LIST.children_of(id)?;
        for child_id in children {
            Self::collect_order(child_id, order)?;
        }
        order.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        // OOM-02 déjà fait via try_reserve
        order.push(id);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId};
    use super::super::snapshot::{flags, make_snapshot_name};
    use super::super::snapshot_list::SnapshotList;

    fn push_snap(list: &SnapshotList, id: u64, bytes: u64, parent: Option<u64>, flags: u32) {
        use super::super::snapshot::Snapshot;
        list.register(Snapshot {
            id: SnapshotId(id), epoch_id: EpochId(1),
            parent_id: parent.map(SnapshotId),
            root_blob: BlobId([0u8;32]), created_at: 100 + id,
            n_blobs: 0, total_bytes: bytes, flags,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"t"),
        }).unwrap();
    }

    #[test]
    fn cannot_delete_protected_without_force() {
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, flags::PROTECTED);
        let snap = list.get(SnapshotId(1)).unwrap();
        let opts = DeleteOptions::default();
        let err = SnapshotDeleter::check_preconditions(&snap, opts);
        assert!(matches!(err, Err(ExofsError::InvalidState)));
    }

    #[test]
    fn force_bypasses_protected() {
        let snap = super::super::snapshot::Snapshot {
            id: SnapshotId(1), epoch_id: EpochId(1), parent_id: None,
            root_blob: BlobId([0u8;32]), created_at: 0, n_blobs: 0,
            total_bytes: 0, flags: flags::PROTECTED,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"t"),
        };
        let opts = DeleteOptions { force: true, ..Default::default() };
        assert!(SnapshotDeleter::check_preconditions(&snap, opts).is_ok());
    }

    #[test]
    fn deny_reason_has_children() {
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, 0);
        push_snap(&list, 2, 0, Some(1), 0);
        let reason = SnapshotDeleter::deny_reason(SnapshotId(1), DeleteOptions::default());
        assert_eq!(reason, Some(DeleteDenyReason::HasChildren));
    }

    #[test]
    fn estimate_cascade_freed_correct() {
        let list = SnapshotList::new_const();
        push_snap(&list, 10, 1000, None, 0);
        push_snap(&list, 11, 2000, Some(10), 0);
        let freed = SnapshotDeleter::estimate_cascade_freed(SnapshotId(10)).unwrap();
        assert_eq!(freed, 3000);
    }

    #[test]
    fn deletion_order_children_first() {
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, 0);
        push_snap(&list, 2, 0, Some(1), 0);
        push_snap(&list, 3, 0, Some(2), 0);
        let order = SnapshotDeleter::deletion_order(SnapshotId(1)).unwrap();
        // 3 doit apparaître avant 2, qui doit apparaître avant 1
        let pos = |id: u64| order.iter().position(|s| s.0 == id).unwrap();
        assert!(pos(3) < pos(2));
        assert!(pos(2) < pos(1));
    }
}
