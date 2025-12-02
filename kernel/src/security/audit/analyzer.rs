//! Audit Analysis
//!
//! Real-time security event analysis with pattern detection

use super::logger::{get_all_events, AuditEvent, AuditEventType};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Threat level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreatLevel {
    None = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

/// Analyze events for suspicious patterns
pub fn analyze_events(events: &[AuditEvent]) -> ThreatLevel {
    let mut denied_count = 0;
    let mut violation_count = 0;
    let mut auth_failures = 0;

    for event in events.iter().rev().take(100) {
        if !event.success {
            denied_count += 1;
        }
        if event.event_type == AuditEventType::PolicyViolation {
            violation_count += 1;
        }
        if event.event_type == AuditEventType::AuthenticationFailure {
            auth_failures += 1;
        }
    }

    // Enhanced heuristics
    if violation_count > 10 || auth_failures > 15 {
        ThreatLevel::Critical
    } else if denied_count > 50 {
        ThreatLevel::High
    } else if denied_count > 20 || auth_failures > 5 {
        ThreatLevel::Medium
    } else if denied_count > 5 {
        ThreatLevel::Low
    } else {
        ThreatLevel::None
    }
}

/// Detect repeated access attempts (brute force)
pub fn detect_brute_force(events: &[AuditEvent], subject: u32) -> bool {
    let failures = events
        .iter()
        .rev()
        .take(20)
        .filter(|e| e.subject == subject && !e.success)
        .count();

    failures > 5
}

/// Get top offenders by denied access count
pub fn get_top_offenders(events: &[AuditEvent], count: usize) -> Vec<(u32, usize)> {
    let mut offenders: BTreeMap<u32, usize> = BTreeMap::new();

    for event in events {
        if !event.success && event.id > 0 {
            *offenders.entry(event.subject).or_insert(0) += 1;
        }
    }

    let mut sorted: Vec<_> = offenders.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(count);
    sorted
}

/// Detect time-based anomalies
pub fn detect_time_anomaly(events: &[AuditEvent], subject: u32) -> bool {
    let subject_events: Vec<_> = events
        .iter()
        .filter(|e| e.subject == subject && e.id > 0)
        .collect();

    if subject_events.len() < 10 {
        return false;
    }

    // Check for burst of events (>10 events in short time window)
    let mut count = 0;
    let mut prev_ts = 0u64;

    for event in subject_events.iter().rev().take(20) {
        if prev_ts > 0 {
            let delta = prev_ts.saturating_sub(event.timestamp);
            // If delta < 1M TSC cycles (~0.3ms at 3GHz), it's very fast
            if delta < 1_000_000 {
                count += 1;
            }
        }
        prev_ts = event.timestamp;
    }

    count > 10
}

/// Get event statistics
pub fn get_statistics(events: &[AuditEvent]) -> AuditStatistics {
    let valid_events: Vec<_> = events.iter().filter(|e| e.id > 0).collect();

    let mut by_type: BTreeMap<u8, usize> = BTreeMap::new();
    let mut success_count = 0;
    let mut failure_count = 0;

    for event in &valid_events {
        *by_type.entry(event.event_type as u8).or_insert(0) += 1;
        if event.success {
            success_count += 1;
        } else {
            failure_count += 1;
        }
    }

    AuditStatistics {
        total_events: valid_events.len(),
        success_count,
        failure_count,
        unique_subjects: valid_events
            .iter()
            .map(|e| e.subject)
            .collect::<alloc::collections::BTreeSet<_>>()
            .len(),
        event_types_count: by_type.len(),
    }
}

#[derive(Debug, Clone)]
pub struct AuditStatistics {
    pub total_events: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub unique_subjects: usize,
    pub event_types_count: usize,
}
