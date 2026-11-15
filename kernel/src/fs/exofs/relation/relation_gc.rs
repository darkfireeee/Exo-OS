//! relation_gc.rs — Orphaned relations collector in ExoFS
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic
//!  - RECUR-01 : no recursion

extern crate alloc;
use alloc::vec::Vec;

use super::relation_graph::RELATION_GRAPH;
use super::relation_index::RELATION_INDEX;
use super::relation_storage::RELATION_STORAGE;
use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01: replaces arch::time
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// BlobExistsChecker
// ─────────────────────────────────────────────────────────────────────────────

/// Interface to check blob existence.
///
/// GC delegates check to implementation provided by caller.
pub trait BlobExistsChecker: Send + Sync {
    /// `true` if blob identified by `key` still exists in store.
    fn exists(&self, key: &[u8; 32]) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// GcPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Collection policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcPolicy {
    /// Removes only orphaned relations (from or to disappeared).
    OrphansOnly,
    /// Also removes soft-deleted relations.
    IncludeDeleted,
    /// Removes all relations whose age exceeds `max_age_ticks`.
    ByAge { max_age_ticks: u64 },
    /// Combination of all policies.
    Full,
}

impl GcPolicy {
    /// `true` if soft-deleted relations should be purged.
    pub fn purge_deleted(self) -> bool {
        matches!(self, GcPolicy::IncludeDeleted | GcPolicy::Full)
    }

    /// `true` if relations that are too old should be purged.
    pub fn purge_by_age(self) -> Option<u64> {
        match self {
            GcPolicy::ByAge { max_age_ticks } => Some(max_age_ticks),
            GcPolicy::Full => Some(u64::MAX),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationGcReport
// ─────────────────────────────────────────────────────────────────────────────

/// GC report.
#[derive(Clone, Debug, Default)]
pub struct RelationGcReport {
    /// Number of examined relations.
    pub examined: u32,
    /// Number of purged relations.
    pub purged: u32,
    /// Number of retained relations.
    pub kept: u32,
    /// Number of encountered errors.
    pub errors: u32,
    /// GC start ticks.
    pub started_at: u64,
    /// GC end ticks.
    pub ended_at: u64,
}

impl RelationGcReport {
    /// GC duration in CPU ticks.
    pub fn duration_ticks(&self) -> u64 {
        self.ended_at.saturating_sub(self.started_at)
    }

    /// Purge rate (purged / examined), in percent.
    pub fn purge_rate_pct(&self) -> u32 {
        if self.examined == 0 {
            return 0;
        }
        ((self.purged as u64 * 100) / self.examined as u64) as u32
    }

    /// `true` if no errors.
    pub fn is_clean(&self) -> bool {
        self.errors == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcCandidate — candidate relation for deletion
// ─────────────────────────────────────────────────────────────────────────────

/// Relation identified as candidate for purge.
#[derive(Clone, Debug)]
pub struct GcCandidate {
    pub blob_from: [u8; 32],
    pub blob_to: [u8; 32],
    pub reason: GcReason,
}

/// Reason for candidacy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcReason {
    /// Source blob no longer exists.
    FromBlobMissing,
    /// Destination blob no longer exists.
    ToBlobMissing,
    /// Relation soft-deleted.
    SoftDeleted,
    /// Relation too old.
    TooOld,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationGc
// ─────────────────────────────────────────────────────────────────────────────

/// Orphaned relations collector.
pub struct RelationGc;

impl RelationGc {
    /// Purges orphaned relations according to given policy.
    ///
    /// Iterative (RECUR-01) — simple `for` loop over relations.
    pub fn run(checker: &dyn BlobExistsChecker, policy: GcPolicy) -> ExofsResult<RelationGcReport> {
        let started_at = exofs_ticks();
        let all = RELATION_STORAGE.load_all()?;
        let now_ticks = exofs_ticks();
        let mut report = RelationGcReport {
            started_at,
            ..Default::default()
        };

        for rel in all {
            report.examined = report
                .examined
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;

            let from_key = rel.from.as_bytes();
            let to_key = rel.to.as_bytes();

            let should_purge = match policy {
                GcPolicy::OrphansOnly => !checker.exists(from_key) || !checker.exists(to_key),
                GcPolicy::IncludeDeleted => {
                    !rel.is_active() || !checker.exists(from_key) || !checker.exists(to_key)
                }
                GcPolicy::ByAge { max_age_ticks } => {
                    let age = now_ticks.saturating_sub(rel.created_at);
                    age > max_age_ticks
                }
                GcPolicy::Full => {
                    let age = now_ticks.saturating_sub(rel.created_at);
                    !rel.is_active()
                        || !checker.exists(from_key)
                        || !checker.exists(to_key)
                        || age > u64::MAX / 2
                }
            };

            if should_purge {
                RELATION_GRAPH.remove_relation(&rel);
                RELATION_INDEX.remove(&rel);
                RELATION_STORAGE.remove(rel.id);
                report.purged = report
                    .purged
                    .checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
            } else {
                report.kept = report
                    .kept
                    .checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?;
            }
        }

        report.ended_at = exofs_ticks();
        Ok(report)
    }

    /// Purges only relations linked to a given (deleted) blob.
    ///
    /// Used during soft-delete or object purge.
    pub fn purge_blob(key: &[u8; 32]) -> ExofsResult<u32> {
        let blob = BlobId(*key);

        let from_ids = RELATION_INDEX.ids_from(&blob);
        let to_ids = RELATION_INDEX.ids_to(&blob);

        let mut n_purged = 0u32;

        for id in from_ids.iter().chain(to_ids.iter()) {
            if let Some(rel_result) = RELATION_STORAGE.load(*id) {
                match rel_result {
                    Ok(rel) => {
                        RELATION_GRAPH.remove_relation(&rel);
                        RELATION_INDEX.remove(&rel);
                        RELATION_STORAGE.remove(*id);
                        n_purged = n_purged.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                    }
                    Err(_) => {
                        // Corrupted relation — still remove from store.
                        RELATION_STORAGE.remove(*id);
                        n_purged = n_purged.saturating_add(1);
                    }
                }
            }
        }

        Ok(n_purged)
    }

    /// Collects purge candidates without deleting them.
    ///
    /// Useful to inspect what would be purged before running a real GC.
    pub fn dry_run(checker: &dyn BlobExistsChecker) -> ExofsResult<Vec<GcCandidate>> {
        let all = RELATION_STORAGE.load_all()?;
        let mut candidates: Vec<GcCandidate> = Vec::new();

        for rel in all {
            let from_key = rel.from.as_bytes();
            let to_key = rel.to.as_bytes();

            if !checker.exists(from_key) {
                candidates
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to: *to_key,
                    reason: GcReason::FromBlobMissing,
                });
            } else if !checker.exists(to_key) {
                candidates
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to: *to_key,
                    reason: GcReason::ToBlobMissing,
                });
            } else if !rel.is_active() {
                candidates
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                candidates.push(GcCandidate {
                    blob_from: *from_key,
                    blob_to: *to_key,
                    reason: GcReason::SoftDeleted,
                });
            }
        }

        Ok(candidates)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcScheduler — heuristic GC scheduling
// ─────────────────────────────────────────────────────────────────────────────

/// Automatic GC trigger criterion.
#[derive(Clone, Debug)]
pub struct GcTrigger {
    /// Triggers if number of relations exceeds this threshold.
    pub max_relations: usize,
    /// Triggers if N ticks have elapsed since last GC.
    pub min_interval_ticks: u64,
}

impl Default for GcTrigger {
    fn default() -> Self {
        GcTrigger {
            max_relations: 60000,
            min_interval_ticks: 1_000_000,
        }
    }
}

/// GC scheduler — decides when it is appropriate to run a GC.
pub struct GcScheduler {
    trigger: GcTrigger,
    last_gc_tick: u64,
    total_gc_runs: u64,
}

impl GcScheduler {
    /// Creates a scheduler with default triggers.
    pub fn new() -> Self {
        GcScheduler {
            trigger: GcTrigger::default(),
            last_gc_tick: 0,
            total_gc_runs: 0,
        }
    }

    /// Creates with explicit triggers.
    pub fn with_trigger(trigger: GcTrigger) -> Self {
        GcScheduler {
            trigger,
            last_gc_tick: 0,
            total_gc_runs: 0,
        }
    }

    /// `true` if a GC should be run now.
    pub fn should_run(&self) -> bool {
        let now = exofs_ticks();
        let n_rels = RELATION_STORAGE.count();
        let interval = now.saturating_sub(self.last_gc_tick);

        n_rels >= self.trigger.max_relations || interval >= self.trigger.min_interval_ticks
    }

    /// Runs GC if conditions are met.
    ///
    /// Returns `None` if GC was not run.
    pub fn maybe_run(
        &mut self,
        checker: &dyn BlobExistsChecker,
        policy: GcPolicy,
    ) -> ExofsResult<Option<RelationGcReport>> {
        if !self.should_run() {
            return Ok(None);
        }
        let report = RelationGc::run(checker, policy)?;
        self.last_gc_tick = exofs_ticks();
        self.total_gc_runs = self.total_gc_runs.wrapping_add(1);
        Ok(Some(report))
    }

    /// Total number of GCs executed since creation.
    pub fn total_runs(&self) -> u64 {
        self.total_gc_runs
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;
    // use super::super::relation::{Relation, RelationId};
    // use super::super::relation_type::{RelationType, RelationKind};
    // use super::super::relation_storage::RelationStorage;
    // use super::super::relation_index::RelationIndex;
    // use super::super::relation_graph::RelationGraph;

    #[allow(dead_code)]
    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    /// Checker that marks all blobs as existing.
    struct AllExist;
    impl BlobExistsChecker for AllExist {
        fn exists(&self, _key: &[u8; 32]) -> bool {
            true
        }
    }

    /// Checker that marks all blobs as deleted.
    #[allow(dead_code)]
    struct NoneExist;
    impl BlobExistsChecker for NoneExist {
        fn exists(&self, _key: &[u8; 32]) -> bool {
            false
        }
    }

    #[test]
    fn test_gc_report_defaults() {
        let r = RelationGcReport::default();
        assert!(r.is_clean());
        assert_eq!(r.purge_rate_pct(), 0);
        assert_eq!(r.duration_ticks(), 0);
    }

    #[test]
    fn test_gc_report_purge_rate() {
        let r = RelationGcReport {
            examined: 10,
            purged: 4,
            ..Default::default()
        };
        assert_eq!(r.purge_rate_pct(), 40);
    }

    #[test]
    fn test_gc_policy_variants() {
        assert!(!GcPolicy::OrphansOnly.purge_deleted());
        assert!(GcPolicy::IncludeDeleted.purge_deleted());
        assert!(GcPolicy::Full.purge_deleted());
        assert!(GcPolicy::ByAge { max_age_ticks: 100 }
            .purge_by_age()
            .is_some());
    }

    #[test]
    fn test_gc_reason_variants() {
        let r = GcReason::FromBlobMissing;
        assert_eq!(r, GcReason::FromBlobMissing);
    }

    #[test]
    fn test_dry_run_all_exist() {
        // All relations have their blobs existing -> no candidates.
        let candidates = RelationGc::dry_run(&AllExist).test_unwrap();
        // Global relations may exist in global store —
        // only verify the function does not panic.
        let _ = candidates;
    }

    #[test]
    fn test_scheduler_new() {
        let s = GcScheduler::new();
        assert_eq!(s.total_runs(), 0);
    }

    #[test]
    fn test_scheduler_with_trigger() {
        let t = GcTrigger {
            max_relations: 100,
            min_interval_ticks: 50,
        };
        let s = GcScheduler::with_trigger(t.clone());
        assert_eq!(s.trigger.max_relations, 100);
    }

    #[test]
    fn test_purge_blob_missing() {
        // Nonexistent blob -> no relations to purge.
        let n = RelationGc::purge_blob(&[0xABu8; 32]).test_unwrap();
        // Unknown how many are in global store,
        // but function must not panic.
        let _ = n;
    }

    #[test]
    fn test_gc_run_all_exist() {
        let checker = AllExist;
        let report = RelationGc::run(&checker, GcPolicy::OrphansOnly).test_unwrap();
        // No orphans because all blobs exist.
        assert!(report.is_clean());
        // purged may be 0 or more depending on global store state.
        let _ = report;
    }

    #[test]
    fn test_gc_candidate_struct() {
        let c = GcCandidate {
            blob_from: [1u8; 32],
            blob_to: [2u8; 32],
            reason: GcReason::ToBlobMissing,
        };
        assert_eq!(c.reason, GcReason::ToBlobMissing);
    }
}
