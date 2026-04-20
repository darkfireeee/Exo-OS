//! # timeline — Forensic timeline reconstruction & event correlation
//!
//! Maintains a chronological record of security-relevant events and
//! provides correlation analysis to identify attack patterns across
//! multiple event types and PIDs.
//!
//! ## Design
//! - Static array of up to 4096 timeline entries (ring buffer)
//! - Each entry records timestamp, event type, PID, and event details
//! - Correlation engine links related events (e.g., an exec followed
//!   by a network connection from the same PID)
//! - Query by PID, time range, or event type

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of timeline entries.
const MAX_TIMELINE_ENTRIES: usize = 4096;

/// Maximum number of correlation links.
const MAX_CORRELATION_LINKS: usize = 512;

/// Maximum correlation chain depth.
const MAX_CORRELATION_DEPTH: usize = 8;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Timeline event type discriminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TimelineEventType {
    /// Process execution.
    Exec = 0,
    /// Network connection.
    NetConnect = 1,
    /// Memory anomaly.
    MemAnomaly = 2,
    /// Syscall anomaly.
    SyscallAnomaly = 3,
    /// IPC policy violation.
    IpcViolation = 4,
    /// Port scan detected.
    PortScan = 5,
    /// Data exfiltration detected.
    Exfiltration = 6,
    /// Use-after-free detected.
    UseAfterFree = 7,
    /// Buffer overflow detected.
    BufferOverflow = 8,
    /// Exec chain anomaly.
    ExecChainAnomaly = 9,
    /// Process terminated.
    ProcessExit = 10,
    /// Sandbox violation.
    SandboxViolation = 11,
    /// DNS anomaly.
    DnsAnomaly = 12,
    /// Threat detected (generic).
    ThreatDetected = 13,
    /// Policy rule change.
    PolicyChange = 14,
    /// Forensic dump created.
    ForensicDump = 15,
}

impl TimelineEventType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Exec,
            1 => Self::NetConnect,
            2 => Self::MemAnomaly,
            3 => Self::SyscallAnomaly,
            4 => Self::IpcViolation,
            5 => Self::PortScan,
            6 => Self::Exfiltration,
            7 => Self::UseAfterFree,
            8 => Self::BufferOverflow,
            9 => Self::ExecChainAnomaly,
            10 => Self::ProcessExit,
            11 => Self::SandboxViolation,
            12 => Self::DnsAnomaly,
            13 => Self::ThreatDetected,
            14 => Self::PolicyChange,
            15 => Self::ForensicDump,
            _ => Self::ThreatDetected,
        }
    }

    /// Returns the severity level of this event type (0=info, 1=low, 2=medium, 3=high, 4=critical).
    pub fn severity(&self) -> u8 {
        match self {
            Self::Exec | Self::NetConnect | Self::ProcessExit | Self::PolicyChange | Self::ForensicDump => 0,
            Self::SyscallAnomaly | Self::DnsAnomaly => 1,
            Self::ExecChainAnomaly | Self::IpcViolation | Self::SandboxViolation => 2,
            Self::PortScan | Self::MemAnomaly | Self::UseAfterFree | Self::BufferOverflow => 3,
            Self::Exfiltration | Self::ThreatDetected => 4,
        }
    }
}

/// A single timeline entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TimelineEntry {
    /// TSC timestamp of the event.
    pub timestamp: u64,
    /// Event type.
    pub event_type: u8,
    /// Severity level (0–4).
    pub severity: u8,
    /// PID associated with the event.
    pub pid: u32,
    /// Secondary PID (e.g., parent PID, IPC target).
    pub pid2: u32,
    /// Event-specific detail field 1 (e.g., syscall number, port, address low bits).
    pub detail1: u32,
    /// Event-specific detail field 2 (e.g., hash, size, address high bits).
    pub detail2: u32,
    /// Correlation group ID (0 = uncorrelated).
    pub correlation_id: u32,
    /// Entry sequence number.
    pub seq: u64,
}

impl Default for TimelineEntry {
    fn default() -> Self {
        Self {
            timestamp: 0,
            event_type: 0,
            severity: 0,
            pid: 0,
            pid2: 0,
            detail1: 0,
            detail2: 0,
            correlation_id: 0,
            seq: 0,
        }
    }
}

/// A correlation link between two timeline entries.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TimelineCorrelation {
    /// Correlation group ID.
    pub group_id: u32,
    /// Sequence number of the first event.
    pub seq_a: u64,
    /// Sequence number of the second event.
    pub seq_b: u64,
    /// PID shared by both events.
    pub pid: u32,
    /// Correlation type: 0=temporal, 1=causal, 2=pid_link.
    pub corr_type: u8,
    /// Strength of the correlation (0–100).
    pub strength: u8,
    /// Padding.
    pub _pad: [u8; 2],
}

impl Default for TimelineCorrelation {
    fn default() -> Self {
        Self {
            group_id: 0,
            seq_a: 0,
            seq_b: 0,
            pid: 0,
            corr_type: 0,
            strength: 0,
            _pad: [0; 2],
        }
    }
}

// ── TSC read ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Timeline entry ring buffer.
static TIMELINE_BUFFER: Mutex<[TimelineEntry; MAX_TIMELINE_ENTRIES]> = Mutex::new(
    [TimelineEntry::default(); MAX_TIMELINE_ENTRIES],
);

/// Write index into the timeline ring buffer.
static TIMELINE_IDX: AtomicU32 = AtomicU32::new(0);

/// Sequence counter for timeline entries.
static TIMELINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Correlation link table.
static CORRELATION_TABLE: Mutex<[TimelineCorrelation; MAX_CORRELATION_LINKS]> = Mutex::new(
    [TimelineCorrelation::default(); MAX_CORRELATION_LINKS],
);
static CORRELATION_COUNT: AtomicU32 = AtomicU32::new(0);

/// Next correlation group ID.
static NEXT_CORR_ID: AtomicU32 = AtomicU32::new(1);

/// Statistics.
static TOTAL_EVENTS: AtomicU64 = AtomicU64::new(0);
static CRITICAL_EVENTS: AtomicU64 = AtomicU64::new(0);
static CORRELATIONS_FOUND: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Find the most recent event for a PID in the timeline buffer.
fn find_recent_event_for_pid(pid: u32, max_lookback: usize) -> Option<TimelineEntry> {
    let buffer = TIMELINE_BUFFER.lock();
    let head = TIMELINE_IDX.load(Ordering::Acquire) as usize;

    for offset in 0..max_lookback.min(MAX_TIMELINE_ENTRIES) {
        let i = (head + MAX_TIMELINE_ENTRIES - 1 - offset) % MAX_TIMELINE_ENTRIES;
        if buffer[i].pid == pid && buffer[i].timestamp != 0 {
            return Some(buffer[i]);
        }
    }
    None
}

/// Determine if two event types are causally related.
fn are_causally_related(type_a: u8, type_b: u8) -> bool {
    // Exec → NetConnect (process exec followed by network activity)
    if type_a == TimelineEventType::Exec as u8 && type_b == TimelineEventType::NetConnect as u8 {
        return true;
    }
    // Exec → SyscallAnomaly (exec followed by suspicious syscalls)
    if type_a == TimelineEventType::Exec as u8 && type_b == TimelineEventType::SyscallAnomaly as u8 {
        return true;
    }
    // PortScan → Exfiltration (port scan followed by data exfiltration)
    if type_a == TimelineEventType::PortScan as u8 && type_b == TimelineEventType::Exfiltration as u8 {
        return true;
    }
    // SyscallAnomaly → BufferOverflow
    if type_a == TimelineEventType::SyscallAnomaly as u8 && type_b == TimelineEventType::BufferOverflow as u8 {
        return true;
    }
    // MemAnomaly → UseAfterFree
    if type_a == TimelineEventType::MemAnomaly as u8 && type_b == TimelineEventType::UseAfterFree as u8 {
        return true;
    }
    // ExecChainAnomaly → Exfiltration
    if type_a == TimelineEventType::ExecChainAnomaly as u8 && type_b == TimelineEventType::Exfiltration as u8 {
        return true;
    }
    // Exec → IpcViolation
    if type_a == TimelineEventType::Exec as u8 && type_b == TimelineEventType::IpcViolation as u8 {
        return true;
    }
    // IpcViolation → SandboxViolation
    if type_a == TimelineEventType::IpcViolation as u8 && type_b == TimelineEventType::SandboxViolation as u8 {
        return true;
    }
    // DnsAnomaly → Exfiltration
    if type_a == TimelineEventType::DnsAnomaly as u8 && type_b == TimelineEventType::Exfiltration as u8 {
        return true;
    }
    false
}

/// Create a correlation link between two events.
fn create_correlation(
    entry_a: &TimelineEntry,
    entry_b: &TimelineEntry,
    corr_type: u8,
    strength: u8,
) {
    // Determine the correlation group: reuse existing group if either
    // event is already correlated
    let group_id = if entry_a.correlation_id != 0 {
        entry_a.correlation_id
    } else if entry_b.correlation_id != 0 {
        entry_b.correlation_id
    } else {
        let id = NEXT_CORR_ID.fetch_add(1, Ordering::AcqRel);
        id
    };

    let link = TimelineCorrelation {
        group_id,
        seq_a: entry_a.seq,
        seq_b: entry_b.seq,
        pid: entry_a.pid,
        corr_type,
        strength,
        _pad: [0; 2],
    };

    let mut table = CORRELATION_TABLE.lock();
    let count = CORRELATION_COUNT.load(Ordering::Acquire) as usize;
    if count < MAX_CORRELATION_LINKS {
        table[count] = link;
        CORRELATION_COUNT.fetch_add(1, Ordering::Release);
    } else {
        // Evict the weakest correlation
        let mut weakest_idx = 0usize;
        let mut weakest_strength = 255u8;
        for i in 0..MAX_CORRELATION_LINKS {
            if table[i].strength < weakest_strength {
                weakest_strength = table[i].strength;
                weakest_idx = i;
            }
        }
        if strength > weakest_strength {
            table[weakest_idx] = link;
        }
    }

    CORRELATIONS_FOUND.fetch_add(1, Ordering::Relaxed);
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Records a timeline event.
///
/// The event is timestamped, assigned a sequence number, and checked
/// for correlations with recent events from the same PID.
///
/// Returns the sequence number of the recorded event.
pub fn record_timeline_event(
    event_type: TimelineEventType,
    pid: u32,
    pid2: u32,
    detail1: u32,
    detail2: u32,
) -> u64 {
    let now = read_tsc();
    let seq = TIMELINE_SEQ.fetch_add(1, Ordering::AcqRel);
    let severity = event_type.severity();

    let entry = TimelineEntry {
        timestamp: now,
        event_type: event_type as u8,
        severity,
        pid,
        pid2,
        detail1,
        detail2,
        correlation_id: 0,
        seq,
    };

    // Store the entry
    {
        let mut buffer = TIMELINE_BUFFER.lock();
        let idx = TIMELINE_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_TIMELINE_ENTRIES;
        buffer[idx] = entry;
    }

    TOTAL_EVENTS.fetch_add(1, Ordering::Relaxed);
    if severity >= 3 {
        CRITICAL_EVENTS.fetch_add(1, Ordering::Relaxed);
    }

    // Correlation analysis — check recent events from the same PID
    if pid != 0 {
        if let Some(prev) = find_recent_event_for_pid(pid, 64) {
            // Temporal correlation: events from the same PID within 1 second
            let elapsed = now.wrapping_sub(prev.timestamp);
            if elapsed < 3_000_000_000 {
                // Create temporal correlation
                create_correlation(&prev, &entry, 0, 50);

                // Causal correlation
                if are_causally_related(prev.event_type, event_type as u8) {
                    create_correlation(&prev, &entry, 1, 85);
                }
            }

            // PID link correlation: events involving the same PID pair
            if pid2 != 0 && (prev.pid == pid2 || prev.pid2 == pid || prev.pid2 == pid2) {
                create_correlation(&prev, &entry, 2, 70);
            }
        }
    }

    seq
}

/// Queries timeline entries for a specific PID.
///
/// Returns entries from most recent to oldest. Fills `out` buffer
/// and returns the number of entries written.
pub fn query_timeline(pid: u32, out: &mut [TimelineEntry]) -> usize {
    let buffer = TIMELINE_BUFFER.lock();
    let head = TIMELINE_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_TIMELINE_ENTRIES {
        let i = (head + MAX_TIMELINE_ENTRIES - 1 - offset) % MAX_TIMELINE_ENTRIES;
        if buffer[i].timestamp == 0 {
            continue;
        }
        if buffer[i].pid == pid || buffer[i].pid2 == pid {
            if written < out.len() {
                out[written] = buffer[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Queries timeline entries by time range.
///
/// Returns entries with timestamps in `[min_ts, max_ts]`.
pub fn query_timeline_by_time(
    min_ts: u64,
    max_ts: u64,
    out: &mut [TimelineEntry],
) -> usize {
    let buffer = TIMELINE_BUFFER.lock();
    let head = TIMELINE_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_TIMELINE_ENTRIES {
        let i = (head + MAX_TIMELINE_ENTRIES - 1 - offset) % MAX_TIMELINE_ENTRIES;
        if buffer[i].timestamp == 0 {
            continue;
        }
        if buffer[i].timestamp >= min_ts && buffer[i].timestamp <= max_ts {
            if written < out.len() {
                out[written] = buffer[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Queries timeline entries by event type.
pub fn query_timeline_by_type(
    event_type: TimelineEventType,
    out: &mut [TimelineEntry],
) -> usize {
    let buffer = TIMELINE_BUFFER.lock();
    let head = TIMELINE_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_TIMELINE_ENTRIES {
        let i = (head + MAX_TIMELINE_ENTRIES - 1 - offset) % MAX_TIMELINE_ENTRIES;
        if buffer[i].timestamp == 0 {
            continue;
        }
        if buffer[i].event_type == event_type as u8 {
            if written < out.len() {
                out[written] = buffer[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Correlates events across PIDs for a given time window.
///
/// Finds events from different PIDs that occur within the specified
/// time window and may be part of a coordinated attack.
///
/// Returns the number of correlation links found.
pub fn correlate_events(time_window_tsc: u64, out: &mut [TimelineCorrelation]) -> usize {
    let buffer = TIMELINE_BUFFER.lock();
    let head = TIMELINE_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    // Scan recent events for cross-PID correlations
    let mut recent_events = [TimelineEntry::default(); 64];
    let mut recent_count = 0usize;

    for offset in 0..64.min(MAX_TIMELINE_ENTRIES) {
        let i = (head + MAX_TIMELINE_ENTRIES - 1 - offset) % MAX_TIMELINE_ENTRIES;
        if buffer[i].timestamp != 0 {
            recent_events[recent_count] = buffer[i];
            recent_count += 1;
        }
    }

    // Check pairs of events from different PIDs within the time window
    for a in 0..recent_count {
        for b in (a + 1)..recent_count {
            if written >= out.len() {
                break;
            }
            let ea = &recent_events[a];
            let eb = &recent_events[b];

            // Must be different PIDs
            if ea.pid == eb.pid {
                continue;
            }

            // Must be within the time window
            let dt = if ea.timestamp > eb.timestamp {
                ea.timestamp.wrapping_sub(eb.timestamp)
            } else {
                eb.timestamp.wrapping_sub(ea.timestamp)
            };
            if dt > time_window_tsc {
                continue;
            }

            // Must be causally related or both high severity
            let related = are_causally_related(ea.event_type, eb.event_type);
            let both_critical = ea.severity >= 3 && eb.severity >= 3;

            if related || both_critical {
                let strength = if related { 80u8 } else { 40u8 };
                let group_id = NEXT_CORR_ID.fetch_add(1, Ordering::AcqRel);

                out[written] = TimelineCorrelation {
                    group_id,
                    seq_a: ea.seq,
                    seq_b: eb.seq,
                    pid: ea.pid,
                    corr_type: if related { 1 } else { 0 },
                    strength,
                    _pad: [0; 2],
                };
                written += 1;
            }
        }
    }

    written
}

/// Gets the correlation chain for a specific correlation group ID.
///
/// Returns timeline entries linked by the correlation chain.
pub fn get_correlation_chain(group_id: u32, out: &mut [TimelineEntry]) -> usize {
    let corr_table = CORRELATION_TABLE.lock();
    let buffer = TIMELINE_BUFFER.lock();

    // Collect all sequence numbers in this correlation group
    let mut seqs = [0u64; MAX_CORRELATION_DEPTH];
    let mut seq_count = 0usize;

    for i in 0..MAX_CORRELATION_LINKS {
        let link = &corr_table[i];
        if link.group_id == group_id {
            // Add seq_a if not already present
            let mut found_a = false;
            for j in 0..seq_count {
                if seqs[j] == link.seq_a {
                    found_a = true;
                    break;
                }
            }
            if !found_a && seq_count < MAX_CORRELATION_DEPTH {
                seqs[seq_count] = link.seq_a;
                seq_count += 1;
            }

            // Add seq_b
            let mut found_b = false;
            for j in 0..seq_count {
                if seqs[j] == link.seq_b {
                    found_b = true;
                    break;
                }
            }
            if !found_b && seq_count < MAX_CORRELATION_DEPTH {
                seqs[seq_count] = link.seq_b;
                seq_count += 1;
            }
        }
    }

    // Look up entries by sequence number
    let mut written = 0usize;
    for s in 0..seq_count {
        if written >= out.len() {
            break;
        }
        let target_seq = seqs[s];
        for i in 0..MAX_TIMELINE_ENTRIES {
            if buffer[i].seq == target_seq {
                out[written] = buffer[i];
                written += 1;
                break;
            }
        }
    }

    written
}

/// Timeline subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TimelineStats {
    pub total_events: u64,
    pub critical_events: u64,
    pub correlations_found: u64,
    pub active_correlations: u32,
    pub buffer_usage: u32,
}

/// Collects timeline statistics.
pub fn get_timeline_stats() -> TimelineStats {
    let total = TOTAL_EVENTS.load(Ordering::Relaxed);
    TimelineStats {
        total_events: total,
        critical_events: CRITICAL_EVENTS.load(Ordering::Relaxed),
        correlations_found: CORRELATIONS_FOUND.load(Ordering::Relaxed),
        active_correlations: CORRELATION_COUNT.load(Ordering::Relaxed),
        buffer_usage: (total as u32).min(MAX_TIMELINE_ENTRIES as u32),
    }
}

/// Resets the timeline subsystem.
pub fn timeline_init() {
    TIMELINE_IDX.store(0, Ordering::Release);
    TIMELINE_SEQ.store(0, Ordering::Release);
    CORRELATION_COUNT.store(0, Ordering::Release);
    NEXT_CORR_ID.store(1, Ordering::Release);
    TOTAL_EVENTS.store(0, Ordering::Release);
    CRITICAL_EVENTS.store(0, Ordering::Release);
    CORRELATIONS_FOUND.store(0, Ordering::Release);
}
