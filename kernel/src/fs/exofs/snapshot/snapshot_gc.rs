//! SnapshotGc — nettoyage des snapshots expirés/orphelins ExoFS (no_std).

use alloc::vec::Vec;
use crate::arch::time::read_ticks;
use crate::fs::exofs::core::FsError;
use super::snapshot::SnapshotId;
use super::snapshot_list::SNAPSHOT_LIST;
use super::snapshot_delete::SnapshotDeleter;

/// Politique de rétention GC.
#[derive(Clone, Debug)]
pub struct SnapshotRetentionPolicy {
    pub max_count:       u64,   // Nombre max de snapshots conservés.
    pub max_age_ticks:   u64,   // Âge max (0 = désactivé).
    pub max_total_bytes: u64,   // Quota d'espace total (0 = désactivé).
}

impl Default for SnapshotRetentionPolicy {
    fn default() -> Self {
        Self {
            max_count:       128,
            max_age_ticks:   30 * 24 * 3600 * 1_000_000_000u64,
            max_total_bytes: 10 * 1024 * 1024 * 1024, // 10 GiB
        }
    }
}

/// Rapport de GC.
#[derive(Clone, Debug, Default)]
pub struct SnapshotGcReport {
    pub examined:  u32,
    pub deleted:   u32,
    pub protected: u32,
    pub errors:    u32,
}

pub struct SnapshotGc;

impl SnapshotGc {
    pub fn run(policy: &SnapshotRetentionPolicy) -> Result<SnapshotGcReport, FsError> {
        let mut report = SnapshotGcReport::default();
        let now = read_ticks();
        let all_ids = SNAPSHOT_LIST.all_ids();
        report.examined = all_ids.len() as u32;

        // Tri du plus ancien au plus récent.
        let mut with_age: Vec<(SnapshotId, u64)> = all_ids.iter().filter_map(|&id| {
            SNAPSHOT_LIST.get(id).map(|s| (id, s.created_at))
        }).collect();
        with_age.sort_by_key(|&(_, ts)| ts);

        for (id, created_at) in with_age {
            let snap = match SNAPSHOT_LIST.get(id) {
                Some(s) => s,
                None    => continue,
            };
            if snap.is_protected() { report.protected += 1; continue; }

            let mut should_delete = false;

            if policy.max_age_ticks > 0 {
                let age = now.saturating_sub(created_at);
                if age > policy.max_age_ticks { should_delete = true; }
            }

            if SNAPSHOT_LIST.count() > policy.max_count { should_delete = true; }
            if policy.max_total_bytes > 0 && SNAPSHOT_LIST.total_bytes() > policy.max_total_bytes {
                should_delete = true;
            }

            if should_delete {
                match SnapshotDeleter::delete(id) {
                    Ok(_)  => report.deleted += 1,
                    Err(_) => report.errors  += 1,
                }
            }
        }
        Ok(report)
    }
}
