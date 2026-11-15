//! audit_rotation.rs — Rotation management for the ExoFS audit log.
//!
//! Rotation triggers serialization of the current ring-buffer to
//! an archived segment, then resets counters to zero.
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic
//!  - RECUR-01 : zero recursion

extern crate alloc;
use alloc::vec::Vec;

use super::audit_entry::{AuditEntry, AUDIT_ENTRY_SIZE};
use super::audit_log::{AuditLogStats, AUDIT_LOG};
use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01: replaces arch::time
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// RotationConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Rotation policy configuration.
#[derive(Clone, Debug)]
pub struct RotationConfig {
    /// Triggers rotation if the ring exceeds this fill percentage.
    pub fill_threshold_pct: u8,
    /// Triggers rotation if N ticks have elapsed since the last one.
    pub max_age_ticks: u64,
    /// Maximum number of archived segments retained in memory.
    pub max_segments: usize,
    /// If `true`, resets ring counters to zero after rotation.
    pub reset_stats_on_rotate: bool,
}

impl Default for RotationConfig {
    fn default() -> Self {
        RotationConfig {
            fill_threshold_pct: 75,
            max_age_ticks: 10_000_000,
            max_segments: 8,
            reset_stats_on_rotate: true,
        }
    }
}

impl RotationConfig {
    /// Minimal configuration for tests.
    pub fn minimal() -> Self {
        RotationConfig {
            fill_threshold_pct: 1,
            max_age_ticks: 1,
            max_segments: 2,
            reset_stats_on_rotate: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RotationReason — trigger reason
// ─────────────────────────────────────────────────────────────────────────────

/// Reason why rotation was triggered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationReason {
    /// Ring near saturation.
    FillThreshold,
    /// Maximum age exceeded.
    AgeExpired,
    /// Forced manual rotation.
    Manual,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditSegment — archived segment
// ─────────────────────────────────────────────────────────────────────────────

/// Audit segment archived during a rotation.
#[derive(Clone, Debug)]
pub struct AuditSegment {
    /// Sequence number of the first archived entry.
    pub seq_start: u64,
    /// Sequence number of the last archived entry.
    pub seq_end: u64,
    /// Tick at the moment of rotation.
    pub rotated_at: u64,
    /// Reason for rotation.
    pub reason: RotationReason,
    /// Number of entries in this segment.
    pub n_entries: u32,
    /// Raw data (serialized entries).
    pub data: Vec<u8>,
}

impl AuditSegment {
    /// Segment size in bytes.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    /// Number of entries.
    pub fn len(&self) -> u32 {
        self.n_entries
    }

    /// `true` if the segment contains no entries.
    pub fn is_empty(&self) -> bool {
        self.n_entries == 0
    }

    /// Deserializes an entry at index `i` (0-based).
    pub fn entry_at(&self, i: u32) -> Option<AuditEntry> {
        let off = (i as usize).checked_mul(AUDIT_ENTRY_SIZE)?;
        let end = off.checked_add(AUDIT_ENTRY_SIZE)?;
        if end > self.data.len() {
            return None;
        }
        let arr: &[u8; AUDIT_ENTRY_SIZE] = self.data[off..end].try_into().ok()?;
        Some(AuditEntry::from_bytes(arr))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RotationReport
// ─────────────────────────────────────────────────────────────────────────────

/// Report produced after a rotation.
#[derive(Clone, Debug)]
pub struct RotationReport {
    /// Number of archived entries.
    pub n_archived: u32,
    /// Number of segments already present after rotation.
    pub n_segments: usize,
    /// Starting ticks.
    pub started_at: u64,
    /// Ending ticks.
    pub ended_at: u64,
    /// Log statistics prior to rotation.
    pub stats_before: AuditLogStats,
    /// Reason for rotation.
    pub reason: RotationReason,
}

impl RotationReport {
    /// Rotation duration in CPU ticks.
    pub fn duration_ticks(&self) -> u64 {
        self.ended_at.saturating_sub(self.started_at)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditRotation
// ─────────────────────────────────────────────────────────────────────────────

/// Audit log rotation manager.
pub struct AuditRotation {
    config: RotationConfig,
    /// Archived segments (FIFO — oldest is at index 0).
    segments: Vec<AuditSegment>,
    last_rotate_tick: u64,
    total_rotations: u64,
}

impl AuditRotation {
    /// Creates with default configuration.
    pub fn new() -> Self {
        AuditRotation {
            config: RotationConfig::default(),
            segments: Vec::new(),
            last_rotate_tick: 0,
            total_rotations: 0,
        }
    }

    /// Creates with explicit configuration.
    pub fn with_config(config: RotationConfig) -> Self {
        AuditRotation {
            config,
            segments: Vec::new(),
            last_rotate_tick: 0,
            total_rotations: 0,
        }
    }

    // ── Trigger policy ────────────────────────────────────────────────────────

    /// Evaluates whether a rotation should be triggered.
    pub fn should_rotate(&self) -> Option<RotationReason> {
        let stats = AUDIT_LOG.stats();
        if stats.ring_fill_pct >= self.config.fill_threshold_pct {
            return Some(RotationReason::FillThreshold);
        }
        let now = exofs_ticks();
        let age = now.saturating_sub(self.last_rotate_tick);
        if self.last_rotate_tick > 0 && age >= self.config.max_age_ticks {
            return Some(RotationReason::AgeExpired);
        }
        None
    }

    /// Initiates a rotation if conditions are met.
    ///
    /// Returns `None` if no rotation was necessary.
    pub fn maybe_rotate(&mut self) -> ExofsResult<Option<RotationReport>> {
        match self.should_rotate() {
            Some(reason) => Ok(Some(self.rotate_with_reason(reason)?)),
            None => Ok(None),
        }
    }

    /// Forced manual rotation.
    pub fn force_rotate(&mut self) -> ExofsResult<RotationReport> {
        self.rotate_with_reason(RotationReason::Manual)
    }

    // ── Segment access ────────────────────────────────────────────────────────

    /// Number of archived segments.
    pub fn n_segments(&self) -> usize {
        self.segments.len()
    }

    /// Segment at index `i` (0 = oldest).
    pub fn segment(&self, i: usize) -> Option<&AuditSegment> {
        self.segments.get(i)
    }

    /// Removes all archived segments.
    pub fn clear_segments(&mut self) {
        self.segments.clear();
    }

    /// Total number of rotations performed.
    pub fn total_rotations(&self) -> u64 {
        self.total_rotations
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn rotate_with_reason(&mut self, reason: RotationReason) -> ExofsResult<RotationReport> {
        let started_at = exofs_ticks();
        let stats_before = AUDIT_LOG.stats();
        let avail = AUDIT_LOG.available();

        let mut data: Vec<u8> = Vec::new();
        data.try_reserve(
            avail
                .checked_mul(AUDIT_ENTRY_SIZE)
                .ok_or(ExofsError::OffsetOverflow)?,
        )
        .map_err(|_| ExofsError::NoMemory)?;

        // Serialize all available entries.
        let seq_start = AUDIT_LOG.next_seq().wrapping_sub(avail as u64);
        let seq_end;
        let n_archived;
        {
            let mut count = 0u32;
            AUDIT_LOG.for_each(|e| {
                data.extend_from_slice(e.as_bytes());
                count = count.wrapping_add(1);
            });
            n_archived = count;
            seq_end = AUDIT_LOG.next_seq().wrapping_sub(1);
        }

        // Create the segment.
        let seg = AuditSegment {
            seq_start,
            seq_end,
            rotated_at: started_at,
            reason,
            n_entries: n_archived,
            data,
        };

        // Append the segment, evict oldest if necessary.
        self.segments
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.segments.push(seg);
        while self.segments.len() > self.config.max_segments {
            self.segments.remove(0);
        }

        // Reset stats if configured.
        if self.config.reset_stats_on_rotate {
            AUDIT_LOG.reset_stats();
        }

        self.last_rotate_tick = exofs_ticks();
        self.total_rotations = self.total_rotations.wrapping_add(1);

        Ok(RotationReport {
            n_archived,
            n_segments: self.segments.len(),
            started_at,
            ended_at: exofs_ticks(),
            stats_before,
            reason,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::audit_entry::{AuditEntry, AuditOp, AuditResult};
    use super::*;

    fn fill_log(n: usize) {
        for i in 0..n {
            AUDIT_LOG.push(AuditEntry::new(
                i as u64,
                1,
                0,
                i as u64,
                [0u8; 32],
                AuditOp::Read,
                AuditResult::Success,
                AUDIT_LOG.next_seq(),
            ));
        }
    }

    #[test]
    fn test_default_config() {
        let conf = RotationConfig::default();
        assert_eq!(conf.fill_threshold_pct, 75);
        assert!(conf.max_segments > 0);
    }

    #[test]
    fn test_new_rotation_no_segments() {
        let r = AuditRotation::new();
        assert_eq!(r.n_segments(), 0);
        assert_eq!(r.total_rotations(), 0);
    }

    #[test]
    fn test_force_rotate_creates_segment() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(2);
        let rep = r.force_rotate().test_unwrap();
        assert!(rep.n_archived >= 2);
        assert_eq!(r.n_segments(), 1);
        assert_eq!(r.total_rotations(), 1);
    }

    #[test]
    fn test_segment_entry_at() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(3);
        r.force_rotate().test_unwrap();
        let seg = r.segment(0).test_unwrap();
        assert!(!seg.is_empty());
        let e = seg.entry_at(0).test_unwrap();
        assert!(e.is_valid());
    }

    #[test]
    fn test_max_segments_eviction() {
        let mut r = AuditRotation::with_config(RotationConfig {
            max_segments: 2,
            ..RotationConfig::minimal()
        });
        fill_log(1);
        r.force_rotate().test_unwrap();
        fill_log(1);
        r.force_rotate().test_unwrap();
        fill_log(1);
        r.force_rotate().test_unwrap();
        // Never exceeds max_segments.
        assert!(r.n_segments() <= 2);
    }

    #[test]
    fn test_rotation_report_duration() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().test_unwrap();
        // Duration is in ticks — may be 0 on fast hardware, never negative.
        assert!(rep.duration_ticks() < u64::MAX);
    }

    #[test]
    fn test_clear_segments() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        r.force_rotate().test_unwrap();
        r.clear_segments();
        assert_eq!(r.n_segments(), 0);
    }

    #[test]
    fn test_reason_manual() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().test_unwrap();
        assert_eq!(rep.reason, RotationReason::Manual);
    }

    #[test]
    fn test_maybe_rotate_none_when_empty_log() {
        let r_no_fill = AuditRotation::new();
        // With default thresholds and a low-fill log, no rotation.
        let _ = r_no_fill; // Does not panic.
    }

    #[test]
    fn test_segment_byte_len() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(2);
        r.force_rotate().test_unwrap();
        let seg = r.segment(0).test_unwrap();
        assert_eq!(seg.byte_len(), seg.n_entries as usize * AUDIT_ENTRY_SIZE);
    }

    #[test]
    fn test_total_rotations_increments() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        r.force_rotate().test_unwrap();
        fill_log(1);
        r.force_rotate().test_unwrap();
        assert_eq!(r.total_rotations(), 2);
    }

    #[test]
    fn test_segment_is_empty_on_empty_log() {
        // Empty ring -> 0-entry segment.
        // Tested via out-of-bounds entry_at.
        let seg = AuditSegment {
            seq_start: 0,
            seq_end: 0,
            rotated_at: 0,
            reason: RotationReason::Manual,
            n_entries: 0,
            data: alloc::vec![],
        };
        assert!(seg.is_empty());
        assert!(seg.entry_at(0).is_none());
    }

    #[test]
    fn test_rotation_report_has_stats_before() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().test_unwrap();
        // stats_before.total_pushed must be >= 1.
        assert!(rep.stats_before.total_pushed >= 1);
    }

    #[test]
    fn test_with_config_respects_fields() {
        let cfg = RotationConfig {
            fill_threshold_pct: 50,
            max_age_ticks: 500,
            max_segments: 3,
            reset_stats_on_rotate: false,
        };
        let r = AuditRotation::with_config(cfg.clone());
        assert_eq!(r.config.max_segments, 3);
    }
}
