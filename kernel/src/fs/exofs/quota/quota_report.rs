//! QuotaReport — rapport d'utilisation des quotas ExoFS (no_std).

use alloc::vec::Vec;
use alloc::string::String;
use crate::fs::exofs::core::FsError;
use super::quota_tracker::{QUOTA_TRACKER, QuotaKey, QuotaUsage};
use super::quota_policy::QuotaLimits;

/// Snapshot de quota pour une entité.
#[derive(Clone, Debug)]
pub struct QuotaReportEntry {
    pub key:            QuotaKey,
    pub usage:          QuotaUsage,
    pub limits:         QuotaLimits,
    pub pct_bytes:      u8,   // Utilisation en % (0-100+).
    pub pct_blobs:      u8,
    pub pct_inodes:     u8,
    pub at_risk_soft:   bool,
    pub at_risk_hard:   bool,
}

impl QuotaReportEntry {
    fn compute(key: QuotaKey, usage: QuotaUsage, limits: QuotaLimits) -> Self {
        let pct = |used: u64, lim: u64| {
            if lim == 0 || lim == u64::MAX { 0u8 }
            else { ((used * 100) / lim.max(1)).min(255) as u8 }
        };
        let pct_bytes  = pct(usage.bytes_used,  limits.soft_bytes);
        let pct_blobs  = pct(usage.blobs_used,  limits.soft_blobs);
        let pct_inodes = pct(usage.inodes_used, limits.soft_inodes);

        let at_risk_soft = usage.bytes_used  > limits.soft_bytes
                        || usage.blobs_used  > limits.soft_blobs
                        || usage.inodes_used > limits.soft_inodes;
        let at_risk_hard = usage.bytes_used  > limits.hard_bytes
                        || usage.blobs_used  > limits.hard_blobs
                        || usage.inodes_used > limits.hard_inodes;

        Self { key, usage, limits, pct_bytes, pct_blobs, pct_inodes, at_risk_soft, at_risk_hard }
    }
}

/// Rapport global généré.
#[derive(Clone, Debug)]
pub struct QuotaReport {
    pub total_bytes:   u64,
    pub total_blobs:   u64,
    pub n_entities:    usize,
    pub n_at_risk:     usize,
    pub entries:       Vec<QuotaReportEntry>,
}

pub struct QuotaReporter;

impl QuotaReporter {
    /// Génère un rapport d'utilisation pour une liste de clés.
    pub fn generate(
        keys: &[QuotaKey],
        limits_for: impl Fn(QuotaKey) -> QuotaLimits,
    ) -> Result<QuotaReport, FsError> {
        let mut entries = Vec::new();
        entries.try_reserve(keys.len()).map_err(|_| FsError::OutOfMemory)?;

        let mut n_at_risk = 0usize;
        for &key in keys {
            let usage  = QUOTA_TRACKER.get_usage(&key);
            let limits = limits_for(key);
            let entry  = QuotaReportEntry::compute(key, usage, limits);
            if entry.at_risk_soft || entry.at_risk_hard { n_at_risk += 1; }
            entries.push(entry);
        }

        Ok(QuotaReport {
            total_bytes: QUOTA_TRACKER.total_bytes(),
            total_blobs: QUOTA_TRACKER.total_blobs(),
            n_entities:  keys.len(),
            n_at_risk,
            entries,
        })
    }

    /// Retourne un résumé lisible.
    pub fn summary(report: &QuotaReport) -> String {
        let mut s = String::new();
        let _ = alloc::fmt::write(
            &mut s,
            format_args!(
                "QuotaReport: {} entities, {} at risk — {}B used / {} blobs",
                report.n_entities, report.n_at_risk,
                report.total_bytes, report.total_blobs,
            ),
        );
        s
    }
}
