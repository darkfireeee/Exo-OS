//! snapshot_gc.rs — Garbage Collector de snapshots ExoFS
//!
//! Supprime les snapshots expirés selon une politique de rétention
//! (nombre max, âge max, quota global).
//!
//! Règles spec :
//!   OOM-02   : try_reserve avant chaque push
//!   ARITH-02 : checked_add pour accumulations

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};
use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;
use super::snapshot_delete::{SnapshotDeleter, DeleteOptions};

// ─────────────────────────────────────────────────────────────
// Politique de rétention
// ─────────────────────────────────────────────────────────────

/// Politique de rétention des snapshots
#[derive(Debug, Clone, Copy)]
pub struct SnapshotRetentionPolicy {
    /// Nombre maximum de snapshots conservés (0 = illimité)
    pub max_count: usize,
    /// Âge maximum en ticks (0 = illimité)
    pub max_age_ticks: u64,
    /// Octets totaux maximum (0 = illimité)
    pub max_total_bytes: u64,
    /// Ne supprime pas les snapshots protégés même s'ils dépassent le quota
    pub respect_protected: bool,
    /// Supprime récursivement les enfants
    pub cascade: bool,
}

impl Default for SnapshotRetentionPolicy {
    fn default() -> Self {
        Self {
            max_count: 0,
            max_age_ticks: 0,
            max_total_bytes: 0,
            respect_protected: true,
            cascade: false,
        }
    }
}

impl SnapshotRetentionPolicy {
    pub fn max_count(mut self, n: usize) -> Self { self.max_count = n; self }
    pub fn max_age(mut self, ticks: u64) -> Self { self.max_age_ticks = ticks; self }
    pub fn max_bytes(mut self, bytes: u64) -> Self { self.max_total_bytes = bytes; self }

    /// Retourne true si aucune limite n'est définie
    pub fn is_unlimited(&self) -> bool {
        self.max_count == 0 && self.max_age_ticks == 0 && self.max_total_bytes == 0
    }
}

// ─────────────────────────────────────────────────────────────
// Rapport GC
// ─────────────────────────────────────────────────────────────

/// Rapport détaillé d'une passe GC
#[derive(Debug, Clone)]
pub struct SnapshotGcReport {
    /// Nombre de snapshots supprimés
    pub n_deleted: u32,
    /// Octets totaux libérés
    pub bytes_freed: u64,
    /// Snapshots supprimés (ids)
    pub deleted_ids: Vec<SnapshotId>,
    /// Snapshots ignorés (protégés, montés, etc.)
    pub n_skipped: u32,
    /// Nombre d'erreurs non fatales
    pub n_errors: u32,
    /// Durée de la passe (ticks)
    pub duration_ticks: u64,
    /// Politique appliquée
    pub policy: SnapshotRetentionPolicy,
    /// Snapshots restants après GC
    pub remaining_count: usize,
}

impl SnapshotGcReport {
    fn new(policy: SnapshotRetentionPolicy) -> Self {
        Self {
            n_deleted: 0, bytes_freed: 0, deleted_ids: Vec::new(),
            n_skipped: 0, n_errors: 0, duration_ticks: 0, policy,
            remaining_count: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Critères d'éligibilité
// ─────────────────────────────────────────────────────────────

/// Raison pour laquelle un snapshot est candidat à la suppression
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcReason {
    AgeExceeded,
    CountExceeded,
    QuotaExceeded,
}

/// Snapshot candidat à la suppression
#[derive(Debug, Clone, Copy)]
pub struct GcCandidate {
    pub id:          SnapshotId,
    pub created_at:  u64,
    pub total_bytes: u64,
    pub reason:      GcReason,
}

// ─────────────────────────────────────────────────────────────
// SnapshotGc
// ─────────────────────────────────────────────────────────────

pub struct SnapshotGc;

impl SnapshotGc {
    // ── Point d'entrée principal ─────────────────────────────────────

    /// Lance une passe GC selon la politique donnée
    pub fn run(policy: SnapshotRetentionPolicy, now: u64) -> ExofsResult<SnapshotGcReport> {
        if policy.is_unlimited() {
            return Ok(SnapshotGcReport::new(policy));
        }

        let mut report = SnapshotGcReport::new(policy);
        let opts = DeleteOptions {
            cascade: policy.cascade,
            force: !policy.respect_protected,
            skip_mounted: true,
        };

        // ── Phase 1 : Suppression par âge ───────────────────────────
        if policy.max_age_ticks > 0 {
            Self::gc_by_age(policy, now, opts, &mut report)?;
        }

        // ── Phase 2 : Suppression par surplus de count ───────────────
        if policy.max_count > 0 {
            Self::gc_by_count(policy, opts, &mut report)?;
        }

        // ── Phase 3 : Suppression par quota bytes ────────────────────
        if policy.max_total_bytes > 0 {
            Self::gc_by_quota(policy, opts, &mut report)?;
        }

        report.remaining_count = SNAPSHOT_LIST.count();
        Ok(report)
    }

    // ── GC par âge ───────────────────────────────────────────────────

    fn gc_by_age(
        policy: SnapshotRetentionPolicy,
        now: u64,
        opts: DeleteOptions,
        report: &mut SnapshotGcReport,
    ) -> ExofsResult<()> {
        let aged = SNAPSHOT_LIST.older_than(now, policy.max_age_ticks)?;
        for snap_ref in aged {
            if policy.respect_protected && snap_ref.flags & flags::PROTECTED != 0 {
                report.n_skipped = report.n_skipped.saturating_add(1);
                continue;
            }
            match SnapshotDeleter::delete(snap_ref.id, opts) {
                Ok(result) => {
                    report.bytes_freed = report.bytes_freed.saturating_add(result.freed_bytes);
                    report.n_deleted = report.n_deleted.saturating_add(1);
                    report.deleted_ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    report.deleted_ids.push(snap_ref.id);
                }
                Err(_) => {
                    report.n_skipped = report.n_skipped.saturating_add(1);
                    report.n_errors  = report.n_errors.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    // ── GC par surplus de count ──────────────────────────────────────

    fn gc_by_count(
        policy: SnapshotRetentionPolicy,
        opts: DeleteOptions,
        report: &mut SnapshotGcReport,
    ) -> ExofsResult<()> {
        let current = SNAPSHOT_LIST.count();
        if current <= policy.max_count { return Ok(()); }

        let excess = current - policy.max_count;
        // Récupère tous les refs triés par created_at croissant (plus anciens en premier)
        let mut all = SNAPSHOT_LIST.all_refs()?;
        all.sort_by_key(|s| s.created_at);

        let mut deleted = 0usize;
        for snap_ref in all {
            if deleted >= excess { break; }
            if policy.respect_protected && snap_ref.flags & flags::PROTECTED != 0 {
                report.n_skipped = report.n_skipped.saturating_add(1);
                continue;
            }
            // Vérifier que l'id n'est pas déjà supprimé lors de cette passe
            if SNAPSHOT_LIST.get(snap_ref.id).is_err() { continue; }

            match SnapshotDeleter::delete(snap_ref.id, opts) {
                Ok(result) => {
                    report.bytes_freed = report.bytes_freed.saturating_add(result.freed_bytes);
                    report.n_deleted   = report.n_deleted.saturating_add(1);
                    report.deleted_ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    report.deleted_ids.push(snap_ref.id);
                    deleted += 1;
                }
                Err(_) => {
                    report.n_skipped = report.n_skipped.saturating_add(1);
                    report.n_errors  = report.n_errors.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    // ── GC par quota bytes ────────────────────────────────────────────

    fn gc_by_quota(
        policy: SnapshotRetentionPolicy,
        opts: DeleteOptions,
        report: &mut SnapshotGcReport,
    ) -> ExofsResult<()> {
        let mut current_bytes = SNAPSHOT_LIST.total_bytes();
        if current_bytes <= policy.max_total_bytes { return Ok(()); }

        let mut all = SNAPSHOT_LIST.all_refs()?;
        all.sort_by_key(|s| s.created_at); // plus anciens en premier

        for snap_ref in all {
            if current_bytes <= policy.max_total_bytes { break; }
            if policy.respect_protected && snap_ref.flags & flags::PROTECTED != 0 {
                report.n_skipped = report.n_skipped.saturating_add(1);
                continue;
            }
            if SNAPSHOT_LIST.get(snap_ref.id).is_err() { continue; }

            match SnapshotDeleter::delete(snap_ref.id, opts) {
                Ok(result) => {
                    report.bytes_freed = report.bytes_freed.saturating_add(result.freed_bytes);
                    current_bytes = current_bytes.saturating_sub(result.freed_bytes);
                    report.n_deleted = report.n_deleted.saturating_add(1);
                    report.deleted_ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    report.deleted_ids.push(snap_ref.id);
                }
                Err(_) => {
                    report.n_skipped = report.n_skipped.saturating_add(1);
                    report.n_errors  = report.n_errors.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    // ── Analyse pré-GC ──────────────────────────────────────────────

    /// Retourne les candidats éligibles sans les supprimer (dry-run)
    pub fn dry_run(policy: SnapshotRetentionPolicy, now: u64) -> ExofsResult<Vec<GcCandidate>> {
        let mut candidates: Vec<GcCandidate> = Vec::new();

        if policy.max_age_ticks > 0 {
            let aged = SNAPSHOT_LIST.older_than(now, policy.max_age_ticks)?;
            for s in aged {
                if policy.respect_protected && s.flags & flags::PROTECTED != 0 { continue; }
                candidates.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate { id: s.id, created_at: s.created_at, total_bytes: s.total_bytes, reason: GcReason::AgeExceeded });
            }
        }

        if policy.max_count > 0 {
            let excess = SNAPSHOT_LIST.count().saturating_sub(policy.max_count);
            if excess > 0 {
                let mut all = SNAPSHOT_LIST.all_refs()?;
                all.sort_by_key(|s| s.created_at);
                for s in all.iter().take(excess) {
                    if policy.respect_protected && s.flags & flags::PROTECTED != 0 { continue; }
                    candidates.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    candidates.push(GcCandidate { id: s.id, created_at: s.created_at, total_bytes: s.total_bytes, reason: GcReason::CountExceeded });
                }
            }
        }

        Ok(candidates)
    }

    /// Octets estimés libérables selon la politique (sans suppression)
    pub fn estimate_freed(policy: SnapshotRetentionPolicy, now: u64) -> ExofsResult<u64> {
        let candidates = Self::dry_run(policy, now)?;
        let mut total: u64 = 0;
        for c in candidates {
            total = total.checked_add(c.total_bytes).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};
    use super::super::snapshot::{Snapshot, make_snapshot_name};
    use super::super::snapshot_list::SnapshotList;

    fn push_snap_age(list: &SnapshotList, id: u64, created_at: u64, bytes: u64) {
        list.register(Snapshot {
            id: SnapshotId(id), epoch_id: EpochId(1), parent_id: None,
            root_blob: BlobId([0u8;32]), created_at,
            n_blobs: 0, total_bytes: bytes, flags: 0,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"gc-test"),
        }).unwrap();
    }

    #[test]
    fn unlimited_policy_does_nothing() {
        let policy = SnapshotRetentionPolicy::default();
        let report = SnapshotGc::run(policy, 9999).unwrap();
        assert_eq!(report.n_deleted, 0);
    }

    #[test]
    fn dry_run_by_age() {
        let list = SnapshotList::new_const();
        push_snap_age(&list, 1, 100, 0);
        push_snap_age(&list, 2, 200, 0);
        push_snap_age(&list, 3, 8000, 0);
        let policy = SnapshotRetentionPolicy::default().max_age(500);
        let candidates = SnapshotGc::dry_run(policy, 1000).unwrap();
        let ids: alloc::vec::Vec<u64> = candidates.iter().map(|c| c.id.0).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(!ids.contains(&3));
    }

    #[test]
    fn estimate_freed_arithmetic() {
        let list = SnapshotList::new_const();
        push_snap_age(&list, 10, 100, 1024);
        push_snap_age(&list, 11, 200, 2048);
        let policy = SnapshotRetentionPolicy::default().max_age(500);
        let freed = SnapshotGc::estimate_freed(policy, 1000).unwrap();
        assert_eq!(freed, 3072);
    }

    #[test]
    fn gc_protected_skipped() {
        let list = SnapshotList::new_const();
        list.register(Snapshot {
            id: SnapshotId(20), epoch_id: EpochId(1), parent_id: None,
            root_blob: BlobId([0u8;32]), created_at: 10,
            n_blobs: 0, total_bytes: 0, flags: flags::PROTECTED,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"protected"),
        }).unwrap();
        let policy = SnapshotRetentionPolicy { max_age_ticks: 500, respect_protected: true, ..Default::default() };
        let report = SnapshotGc::run(policy, 1000).unwrap();
        assert_eq!(report.n_deleted, 0);
        assert!(report.n_skipped > 0);
    }
}
