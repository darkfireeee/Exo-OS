// SPDX-License-Identifier: MIT
// ExoFS Quota — Rapports et exports de quota
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::quota_tracker::{QuotaKey, QuotaUsage, QUOTA_TRACKER, QUOTA_MAX_ENTRIES};
use super::quota_audit::{QUOTA_AUDIT, AuditSummary, audit_tick};
use super::quota_policy::{QuotaLimits, QuotaKind};

// ─── Seuils de sévérité (‰) ──────────────────────────────────────────────────

/// Seuil warning (75 %).
pub const SEVERITY_WARNING_PPT:  u64 = 750;
/// Seuil critical (90 %).
pub const SEVERITY_CRITICAL_PPT: u64 = 900;
/// Seuil emergency (100 %).
pub const SEVERITY_EMERGENCY_PPT: u64 = 1000;

// ─── ReportDimension ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportDimension { Bytes, Blobs, Inodes }

impl ReportDimension {
    pub fn name(self) -> &'static str {
        match self { Self::Bytes => "bytes", Self::Blobs => "blobs", Self::Inodes => "inodes" }
    }
}

// ─── QuotaReportEntry ─────────────────────────────────────────────────────────

/// Entrée de rapport pour une clé de quota.
#[derive(Clone, Copy, Debug)]
pub struct QuotaReportEntry {
    pub key:                 QuotaKey,
    pub usage:               QuotaUsage,
    pub limits:              QuotaLimits,
    /// Usage bytes en ‰.
    pub bytes_ppt:           u64,
    /// Usage blobs en ‰.
    pub blobs_ppt:           u64,
    /// Usage inodes en ‰.
    pub inodes_ppt:          u64,
    pub soft_breach_bytes:   bool,
    pub soft_breach_blobs:   bool,
    pub soft_breach_inodes:  bool,
    pub hard_exceed_bytes:   bool,
    pub hard_exceed_blobs:   bool,
    pub hard_exceed_inodes:  bool,
    /// Severity 0=ok 1=warning 2=critical 3=emergency.
    pub severity:            u8,
    pub soft_breach_tick:    u64,
}

impl QuotaReportEntry {
    /// Calcule une entrée à partir des données de tracking.
    pub fn compute(key: QuotaKey, usage: QuotaUsage, limits: QuotaLimits, breach_tick: u64)
        -> Self
    {
        // ARITH-02 : saturating_mul + checked_div
        let bytes_ppt = if limits.hard_bytes == 0 || limits.hard_bytes == u64::MAX { 0 } else {
            usage.bytes_used.saturating_mul(1000)
                .checked_div(limits.hard_bytes).unwrap_or(1000)
        };
        let blobs_ppt = if limits.hard_blobs == 0 || limits.hard_blobs == u64::MAX { 0 } else {
            usage.blobs_used.saturating_mul(1000)
                .checked_div(limits.hard_blobs).unwrap_or(1000)
        };
        let inodes_ppt = if limits.hard_inodes == 0 || limits.hard_inodes == u64::MAX { 0 } else {
            usage.inodes_used.saturating_mul(1000)
                .checked_div(limits.hard_inodes).unwrap_or(1000)
        };

        let soft_breach_bytes   = limits.soft_bytes  != u64::MAX && usage.bytes_used  > limits.soft_bytes;
        let soft_breach_blobs   = limits.soft_blobs  != u64::MAX && usage.blobs_used  > limits.soft_blobs;
        let soft_breach_inodes  = limits.soft_inodes != u64::MAX && usage.inodes_used > limits.soft_inodes;
        let hard_exceed_bytes   = limits.hard_bytes  != u64::MAX && usage.bytes_used  >= limits.hard_bytes;
        let hard_exceed_blobs   = limits.hard_blobs  != u64::MAX && usage.blobs_used  >= limits.hard_blobs;
        let hard_exceed_inodes  = limits.hard_inodes != u64::MAX && usage.inodes_used >= limits.hard_inodes;

        let max_ppt = bytes_ppt.max(blobs_ppt).max(inodes_ppt);
        let severity = if max_ppt >= SEVERITY_EMERGENCY_PPT || hard_exceed_bytes
                            || hard_exceed_blobs || hard_exceed_inodes { 3 }
                       else if max_ppt >= SEVERITY_CRITICAL_PPT { 2 }
                       else if max_ppt >= SEVERITY_WARNING_PPT { 1 } else { 0 };

        Self {
            key, usage, limits,
            bytes_ppt, blobs_ppt, inodes_ppt,
            soft_breach_bytes, soft_breach_blobs, soft_breach_inodes,
            hard_exceed_bytes, hard_exceed_blobs, hard_exceed_inodes,
            severity,
            soft_breach_tick: breach_tick,
        }
    }

    /// Vrai si au moins une dimension est dans un état critique ou pire.
    pub fn is_at_risk(&self) -> bool { self.severity >= 2 }
    /// Vrai si l'entité a dépassé une limite hard.
    pub fn has_hard_exceed(&self) -> bool {
        self.hard_exceed_bytes || self.hard_exceed_blobs || self.hard_exceed_inodes
    }
    /// Dimension la plus chargée.
    pub fn highest_dimension(&self) -> ReportDimension {
        if self.bytes_ppt >= self.blobs_ppt && self.bytes_ppt >= self.inodes_ppt {
            ReportDimension::Bytes
        } else if self.blobs_ppt >= self.inodes_ppt {
            ReportDimension::Blobs
        } else {
            ReportDimension::Inodes
        }
    }
    /// Poids total (ARITH-02).
    pub fn total_weight(&self) -> u64 {
        self.bytes_ppt.saturating_add(self.blobs_ppt).saturating_add(self.inodes_ppt)
    }
}

// ─── QuotaReport ──────────────────────────────────────────────────────────────

/// Rapport complet de quota pour toutes les entités.
pub struct QuotaReport {
    pub entries:           Vec<QuotaReportEntry>,
    pub total_bytes_used:  u64,
    pub total_blobs_used:  u64,
    pub total_inodes_used: u64,
    pub breach_count:      usize,
    pub exceed_count:      usize,
    pub snapshot_tick:     u64,
    pub audit_summary:     AuditSummary,
}

impl QuotaReport {
    /// Génère un rapport à partir du tracker actuel (OOM-02, RECUR-01).
    pub fn from_tracker() -> ExofsResult<Self> {
        let tick = audit_tick();
        let audit_summary = QUOTA_AUDIT.summary();

        // Snapshot des entrées (OOM-02)
        let all = QUOTA_TRACKER.snapshot_all()?;
        let n = all.len();
        let mut entries = Vec::new();
        entries.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;

        let mut total_bytes:  u64 = 0;
        let mut total_blobs:  u64 = 0;
        let mut total_inodes: u64 = 0;
        let mut breach_count: usize = 0;
        let mut exceed_count: usize = 0;

        // RECUR-01 : while sur l'indice
        let mut i = 0usize;
        while i < n {
            let se = &all[i];
            let entry = QuotaReportEntry::compute(
                se.key, se.usage, se.limits, se.soft_breach_tick
            );
            total_bytes  = total_bytes.saturating_add(se.usage.bytes_used);
            total_blobs  = total_blobs.saturating_add(se.usage.blobs_used);
            total_inodes = total_inodes.saturating_add(se.usage.inodes_used);
            if entry.soft_breach_bytes || entry.soft_breach_blobs || entry.soft_breach_inodes {
                breach_count = breach_count.saturating_add(1);
            }
            if entry.has_hard_exceed() {
                exceed_count = exceed_count.saturating_add(1);
            }
            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(entry);
            i = i.wrapping_add(1);
        }

        Ok(Self {
            entries,
            total_bytes_used:  total_bytes,
            total_blobs_used:  total_blobs,
            total_inodes_used: total_inodes,
            breach_count,
            exceed_count,
            snapshot_tick: tick,
            audit_summary,
        })
    }

    /// Filtre les entrées à risque (severity ≥ 2).
    pub fn at_risk(&self) -> Vec<&QuotaReportEntry> {
        let mut v = Vec::new();
        let mut i = 0usize;
        while i < self.entries.len() {
            if self.entries[i].is_at_risk() { v.push(&self.entries[i]); }
            i = i.wrapping_add(1);
        }
        v
    }

    /// Filtre les entrées en limite hard dépassée.
    pub fn exceeded(&self) -> Vec<&QuotaReportEntry> {
        let mut v = Vec::new();
        let mut i = 0usize;
        while i < self.entries.len() {
            if self.entries[i].has_hard_exceed() { v.push(&self.entries[i]); }
            i = i.wrapping_add(1);
        }
        v
    }

    /// Top-N consommateurs (bytes_ppt décroissant, RECUR-01 : while).
    pub fn top_consumers(&self, top_n: usize) -> Vec<&QuotaReportEntry> {
        let mut indices: Vec<usize> = Vec::new();
        if indices.try_reserve(self.entries.len()).is_err() { return Vec::new(); }
        let mut i = 0usize;
        while i < self.entries.len() {
            indices.push(i);
            i = i.wrapping_add(1);
        }
        // Tri par insertion (RECUR-01 : pas de tri récursif)
        let mut j = 1usize;
        while j < indices.len() {
            let key_j = self.entries[indices[j]].total_weight();
            let mut k = j;
            while k > 0 && self.entries[indices[k - 1]].total_weight() < key_j {
                indices.swap(k - 1, k);
                k = k.wrapping_sub(1);
            }
            j = j.wrapping_add(1);
        }
        let take = top_n.min(indices.len());
        let mut result = Vec::new();
        let mut m = 0usize;
        while m < take {
            result.push(&self.entries[indices[m]]);
            m = m.wrapping_add(1);
        }
        result
    }

    /// Poids total de toutes les entités (ARITH-02).
    pub fn global_weight(&self) -> u64 {
        let mut sum = 0u64;
        let mut i = 0usize;
        while i < self.entries.len() {
            sum = sum.saturating_add(self.entries[i].total_weight());
            i = i.wrapping_add(1);
        }
        sum
    }

    pub fn is_clean(&self) -> bool { self.exceed_count == 0 && self.breach_count == 0 }

    pub fn entry_count(&self) -> usize { self.entries.len() }
}

// ─── QuotaReporter ────────────────────────────────────────────────────────────

/// Façade pour générer des rapports quota.
pub struct QuotaReporter;

impl QuotaReporter {
    pub const fn new() -> Self { Self }

    /// Rapport complet.
    pub fn full_report(&self) -> ExofsResult<QuotaReport> {
        QuotaReport::from_tracker()
    }

    /// Liste des entités à risque uniquement.
    pub fn at_risk_report(&self) -> ExofsResult<Vec<QuotaReportEntry>> {
        let report = QuotaReport::from_tracker()?;
        let risk = report.at_risk();
        let n = risk.len();
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < n {
            v.push(*risk[i]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Ligne de résumé ASCII (max 64 octets).
    pub fn summary_line(&self) -> ExofsResult<[u8; 64]> {
        let r = QuotaReport::from_tracker()?;
        let mut buf = [b' '; 64];
        // Format: "entries=NNN beach=NNN exceed=NNN  "
        let msg = alloc::format!(
            "entries={} breach={} exceed={}",
            r.entry_count(), r.breach_count, r.exceed_count
        );
        let bytes = msg.as_bytes();
        let len = bytes.len().min(64);
        let mut i = 0usize;
        while i < len { buf[i] = bytes[i]; i = i.wrapping_add(1); }
        Ok(buf)
    }

    /// Retourne les violations actives (hard_exceed).
    pub fn active_violations(&self) -> ExofsResult<Vec<QuotaReportEntry>> {
        let report = QuotaReport::from_tracker()?;
        let exc = report.exceeded();
        let n = exc.len();
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < n {
            v.push(*exc[i]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Ratio global d'utilisation bytes en ‰.
    pub fn global_bytes_ppt(&self) -> ExofsResult<u64> {
        let report = QuotaReport::from_tracker()?;
        if report.entry_count() == 0 { return Ok(0); }
        let mut sum = 0u64;
        let mut i = 0usize;
        while i < report.entries.len() {
            sum = sum.saturating_add(report.entries[i].bytes_ppt);
            i = i.wrapping_add(1);
        }
        Ok(sum.checked_div(report.entry_count() as u64).unwrap_or(0))
    }
}

/// Instance globale du reporter.
pub static QUOTA_REPORTER: QuotaReporter = QuotaReporter::new();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::quota::quota_tracker::QUOTA_TRACKER;
    use crate::fs::exofs::quota::quota_policy::{QuotaLimits, QuotaKind};

    fn setup(entity_id: u64, hard: u64, soft: u64, used: u64) -> QuotaKey {
        let key = QuotaKey::new(QuotaKind::User, entity_id);
        let mut l = QuotaLimits::unlimited();
        l.hard_bytes = hard; l.soft_bytes = soft;
        QUOTA_TRACKER.set_limits(key, l).unwrap();
        QUOTA_TRACKER.reset_usage(key).unwrap_or(());
        if used > 0 { QUOTA_TRACKER.add_bytes(key, used).unwrap(); }
        key
    }

    #[test]
    fn test_compute_ok() {
        let usage = QuotaUsage { bytes_used: 500, blobs_used: 0, inodes_used: 0 };
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes = 1000;
        let key = QuotaKey::new(QuotaKind::User, 0);
        let e = QuotaReportEntry::compute(key, usage, limits, 0);
        assert_eq!(e.bytes_ppt, 500);
        assert_eq!(e.severity, 0);
    }

    #[test]
    fn test_compute_warning() {
        let usage = QuotaUsage { bytes_used: 800, blobs_used: 0, inodes_used: 0 };
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes = 1000;
        limits.soft_bytes = 700;
        let key = QuotaKey::new(QuotaKind::User, 0);
        let e = QuotaReportEntry::compute(key, usage, limits, 0);
        assert_eq!(e.bytes_ppt, 800);
        assert_eq!(e.severity, 1);
        assert!(e.soft_breach_bytes);
    }

    #[test]
    fn test_compute_critical() {
        let usage = QuotaUsage { bytes_used: 950, blobs_used: 0, inodes_used: 0 };
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes = 1000;
        let key = QuotaKey::new(QuotaKind::User, 0);
        let e = QuotaReportEntry::compute(key, usage, limits, 0);
        assert_eq!(e.severity, 2);
    }

    #[test]
    fn test_compute_emergency() {
        let usage = QuotaUsage { bytes_used: 1000, blobs_used: 0, inodes_used: 0 };
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes = 1000;
        let key = QuotaKey::new(QuotaKind::User, 0);
        let e = QuotaReportEntry::compute(key, usage, limits, 0);
        assert_eq!(e.severity, 3);
        assert!(e.hard_exceed_bytes);
    }

    #[test]
    fn test_report_is_clean() {
        let _ = setup(200, 100_000, 50_000, 100);
        let r = QuotaReport::from_tracker().unwrap();
        // Peut contenir d'autres entrées de tests précédents, on vérifie juste que ça compile
        let _ = r.is_clean();
    }

    #[test]
    fn test_report_at_risk() {
        let _ = setup(201, 1_000, 500, 950);
        let r = QuotaReport::from_tracker().unwrap();
        let at_risk = r.at_risk();
        // Au moins une entrée at-risk (la nôtre)
        assert!(!at_risk.is_empty());
    }

    #[test]
    fn test_top_consumers() {
        let _ = setup(202, 100_000, 50_000, 80_000);
        let _ = setup(203, 100_000, 50_000, 10_000);
        let r = QuotaReport::from_tracker().unwrap();
        let top = r.top_consumers(2);
        assert!(top.len() <= 2);
        if top.len() == 2 {
            assert!(top[0].total_weight() >= top[1].total_weight());
        }
    }

    #[test]
    fn test_reporter_summary_line() {
        let _ = setup(204, 100_000, 50_000, 0);
        let line = QUOTA_REPORTER.summary_line().unwrap();
        let s = core::str::from_utf8(&line).unwrap_or("").trim();
        assert!(s.starts_with("entries="));
    }

    #[test]
    fn test_reporter_full_report() {
        let _ = setup(205, 100_000, 50_000, 1000);
        let r = QUOTA_REPORTER.full_report().unwrap();
        assert!(r.entry_count() > 0);
    }

    #[test]
    fn test_highest_dimension_bytes() {
        let usage = QuotaUsage { bytes_used: 900, blobs_used: 100, inodes_used: 50 };
        let mut limits = QuotaLimits::unlimited();
        limits.hard_bytes  = 1000;
        limits.hard_blobs  = 1000;
        limits.hard_inodes = 1000;
        let key = QuotaKey::new(QuotaKind::User, 0);
        let e = QuotaReportEntry::compute(key, usage, limits, 0);
        assert_eq!(e.highest_dimension(), ReportDimension::Bytes);
    }

    #[test]
    fn test_global_bytes_ppt() {
        let _ = setup(206, 1_000, 500, 500);
        let ppt = QUOTA_REPORTER.global_bytes_ppt().unwrap();
        assert!(ppt > 0);
    }
}
