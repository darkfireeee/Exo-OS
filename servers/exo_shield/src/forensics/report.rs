//! # report — Forensic report generation & serialization
//!
//! Generates structured forensic reports from the collected security
//! event data. Reports include threat summaries, incident details,
//! and remediation recommendations.
//!
//! ## Design
//! - All structures are `#[repr(C)]` with fixed-size arrays
//! - Serialization writes a compact binary format into a caller-provided
//!   buffer (no heap allocation)
//! - Reports are self-contained snapshots of the security state

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of threat categories in a report.
const MAX_THREATS: usize = 8;

/// Maximum number of incident details in a report.
const MAX_INCIDENTS: usize = 16;

/// Maximum number of recommendations in a report.
const MAX_RECOMMENDATIONS: usize = 8;

/// Report magic number for binary format identification.
const REPORT_MAGIC_VALUE: u32 = 0xF05E_F1LE;

/// Report format version.
const REPORT_VERSION: u32 = 1;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Threat severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreatLevel {
    /// No threat — informational.
    Info = 0,
    /// Low severity — monitoring recommended.
    Low = 1,
    /// Medium severity — investigation required.
    Medium = 2,
    /// High severity — immediate action required.
    High = 3,
    /// Critical — system compromise suspected.
    Critical = 4,
}

impl ThreatLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Info,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            4 => Self::Critical,
            _ => Self::Info,
        }
    }
}

/// Threat category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreatCategory {
    /// Unknown / uncategorized.
    Unknown = 0,
    /// Malware execution.
    Malware = 1,
    /// Privilege escalation.
    PrivEscalation = 2,
    /// Lateral movement.
    LateralMovement = 3,
    /// Data exfiltration.
    Exfiltration = 4,
    /// Denial of service.
    DoS = 5,
    /// Reconnaissance / probing.
    Recon = 6,
    /// Policy violation.
    PolicyViolation = 7,
}

impl ThreatCategory {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::Malware,
            2 => Self::PrivEscalation,
            3 => Self::LateralMovement,
            4 => Self::Exfiltration,
            5 => Self::DoS,
            6 => Self::Recon,
            7 => Self::PolicyViolation,
            _ => Self::Unknown,
        }
    }
}

/// A single threat summary entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ThreatSummary {
    /// Threat category.
    pub category: u8,
    /// Severity level.
    pub severity: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// Number of events matching this threat.
    pub event_count: u32,
    /// Primary PID involved.
    pub primary_pid: u32,
    /// Secondary PID involved (0 = none).
    pub secondary_pid: u32,
    /// First event timestamp (TSC).
    pub first_seen: u64,
    /// Last event timestamp (TSC).
    pub last_seen: u64,
    /// Short description hash (FNV-1a of description text).
    pub description_hash: u64,
}

impl Default for ThreatSummary {
    fn default() -> Self {
        Self {
            category: 0,
            severity: 0,
            _pad: [0; 2],
            event_count: 0,
            primary_pid: 0,
            secondary_pid: 0,
            first_seen: 0,
            last_seen: 0,
            description_hash: 0,
        }
    }
}

/// A single incident detail entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IncidentDetail {
    /// Event type (from TimelineEventType).
    pub event_type: u8,
    /// Severity of the incident.
    pub severity: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// PID involved.
    pub pid: u32,
    /// Event detail field 1.
    pub detail1: u32,
    /// Event detail field 2.
    pub detail2: u32,
    /// TSC timestamp of the incident.
    pub timestamp: u64,
    /// Correlation group ID (0 = uncorrelated).
    pub correlation_id: u32,
    /// Additional padding.
    pub _pad2: u32,
}

impl Default for IncidentDetail {
    fn default() -> Self {
        Self {
            event_type: 0,
            severity: 0,
            _pad: [0; 2],
            pid: 0,
            detail1: 0,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        }
    }
}

/// A single remediation recommendation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Recommendation {
    /// Recommendation type: 0=kill_process, 1=block_ip, 2=isolate,
    /// 3=add_policy, 4=remove_policy, 5=scan_memory, 6=audit_review,
    /// 7=update_signatures.
    pub rec_type: u8,
    /// Priority: 0=low, 1=medium, 2=high, 3=immediate.
    pub priority: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// Target PID (if applicable).
    pub target_pid: u32,
    /// Target IP or additional parameter.
    pub target_param: u32,
    /// Description hash (FNV-1a of recommendation text).
    pub description_hash: u64,
}

impl Default for Recommendation {
    fn default() -> Self {
        Self {
            rec_type: 0,
            priority: 0,
            _pad: [0; 2],
            target_pid: 0,
            target_param: 0,
            description_hash: 0,
        }
    }
}

/// Complete forensic report.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Report {
    /// Report format version.
    pub version: u32,
    /// TSC timestamp of report generation.
    pub timestamp: u64,
    /// Overall threat level (maximum of all threat severities).
    pub overall_threat_level: u8,
    /// Total number of events analyzed.
    pub total_events_analyzed: u32,
    /// Number of threats in this report.
    pub threat_count: u8,
    /// Number of incidents in this report.
    pub incident_count: u8,
    /// Number of recommendations in this report.
    pub recommendation_count: u8,
    /// Padding.
    pub _pad: [u8; 1],
    /// Threat summaries.
    pub threats: [ThreatSummary; MAX_THREATS],
    /// Incident details.
    pub incidents: [IncidentDetail; MAX_INCIDENTS],
    /// Recommendations.
    pub recommendations: [Recommendation; MAX_RECOMMENDATIONS],
    /// CRC-32 checksum of the report data (computed during serialization).
    pub checksum: u32,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            version: REPORT_VERSION,
            timestamp: 0,
            overall_threat_level: 0,
            total_events_analyzed: 0,
            threat_count: 0,
            incident_count: 0,
            recommendation_count: 0,
            _pad: [0],
            threats: [ThreatSummary::default(); MAX_THREATS],
            incidents: [IncidentDetail::default(); MAX_INCIDENTS],
            recommendations: [Recommendation::default(); MAX_RECOMMENDATIONS],
            checksum: 0,
        }
    }
}

// ── FNV-1a hash ───────────────────────────────────────────────────────────────

#[inline]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

// ── CRC-32 ────────────────────────────────────────────────────────────────────

static CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC_TABLE[idx];
    }
    !crc
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

/// Statistics.
static REPORTS_GENERATED: AtomicU64 = AtomicU64::new(0);
static LAST_REPORT_TIME: AtomicU64 = AtomicU64::new(0);

// ── Public API ────────────────────────────────────────────────────────────────

/// Generates a forensic report from the current security state.
///
/// This function collects data from the hook subsystems and
/// assembles a structured report with:
/// - Threat summaries (aggregated by category)
/// - Incident details (most severe recent events)
/// - Remediation recommendations
///
/// The report is stored in the returned `Report` struct.
pub fn generate_report(
    exec_total: u64,
    exec_denied: u64,
    net_scan_detections: u64,
    net_exfil_detections: u64,
    mem_overflows: u64,
    mem_uaf: u64,
    syscall_rate_anomalies: u64,
    syscall_seq_matches: u64,
    ipc_denied: u64,
) -> Report {
    let mut report = Report::default();
    report.timestamp = read_tsc();
    report.total_events_analyzed = (exec_total + net_scan_detections + net_exfil_detections
        + mem_overflows + mem_uaf + syscall_rate_anomalies + syscall_seq_matches + ipc_denied)
        as u32;

    let mut threat_idx = 0usize;
    let mut max_severity = 0u8;

    // ── Threat: Privilege Escalation (exec denials) ──
    if exec_denied > 0 {
        let severity = if exec_denied > 10 { ThreatLevel::Critical as u8 }
                       else if exec_denied > 3 { ThreatLevel::High as u8 }
                       else { ThreatLevel::Medium as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::PrivEscalation as u8,
                severity,
                _pad: [0; 2],
                event_count: exec_denied as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"exec_denial_escalation"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: Reconnaissance (port scans) ──
    if net_scan_detections > 0 {
        let severity = if net_scan_detections > 5 { ThreatLevel::High as u8 }
                       else { ThreatLevel::Medium as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::Recon as u8,
                severity,
                _pad: [0; 2],
                event_count: net_scan_detections as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"port_scan_recon"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: Data Exfiltration ──
    if net_exfil_detections > 0 {
        let severity = ThreatLevel::Critical as u8;
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::Exfiltration as u8,
                severity,
                _pad: [0; 2],
                event_count: net_exfil_detections as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"data_exfiltration"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: Malware (memory anomalies) ──
    if mem_overflows > 0 || mem_uaf > 0 {
        let total_mem = mem_overflows + mem_uaf;
        let severity = if total_mem > 5 { ThreatLevel::Critical as u8 }
                       else if total_mem > 2 { ThreatLevel::High as u8 }
                       else { ThreatLevel::Medium as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::Malware as u8,
                severity,
                _pad: [0; 2],
                event_count: total_mem as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"memory_anomaly_malware"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: Lateral Movement (syscall sequences) ──
    if syscall_seq_matches > 0 {
        let severity = if syscall_seq_matches > 3 { ThreatLevel::Critical as u8 }
                       else { ThreatLevel::High as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::LateralMovement as u8,
                severity,
                _pad: [0; 2],
                event_count: syscall_seq_matches as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"syscall_sequence_lateral"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: DoS (rate anomalies) ──
    if syscall_rate_anomalies > 0 {
        let severity = if syscall_rate_anomalies > 10 { ThreatLevel::High as u8 }
                       else { ThreatLevel::Medium as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::DoS as u8,
                severity,
                _pad: [0; 2],
                event_count: syscall_rate_anomalies as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"rate_anomaly_dos"),
            };
            threat_idx += 1;
        }
    }

    // ── Threat: Policy Violation (IPC denials) ──
    if ipc_denied > 0 {
        let severity = if ipc_denied > 20 { ThreatLevel::High as u8 }
                       else { ThreatLevel::Low as u8 };
        max_severity = max_severity.max(severity);

        if threat_idx < MAX_THREATS {
            report.threats[threat_idx] = ThreatSummary {
                category: ThreatCategory::PolicyViolation as u8,
                severity,
                _pad: [0; 2],
                event_count: ipc_denied as u32,
                primary_pid: 0,
                secondary_pid: 0,
                first_seen: 0,
                last_seen: 0,
                description_hash: fnv1a_hash(b"ipc_policy_violation"),
            };
            threat_idx += 1;
        }
    }

    report.threat_count = threat_idx as u8;
    report.overall_threat_level = max_severity;

    // ── Incident details ──
    // Populate with the most significant events
    let mut inc_idx = 0usize;

    if mem_overflows > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 8, // BufferOverflow
            severity: ThreatLevel::High as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: mem_overflows as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if mem_uaf > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 7, // UseAfterFree
            severity: ThreatLevel::High as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: mem_uaf as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if net_exfil_detections > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 6, // Exfiltration
            severity: ThreatLevel::Critical as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: net_exfil_detections as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if net_scan_detections > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 5, // PortScan
            severity: ThreatLevel::High as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: net_scan_detections as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if syscall_seq_matches > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 3, // SyscallAnomaly
            severity: ThreatLevel::Critical as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: syscall_seq_matches as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if exec_denied > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 0, // Exec
            severity: ThreatLevel::Medium as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: exec_denied as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if ipc_denied > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 4, // IpcViolation
            severity: ThreatLevel::Medium as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: ipc_denied as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    if syscall_rate_anomalies > 0 && inc_idx < MAX_INCIDENTS {
        report.incidents[inc_idx] = IncidentDetail {
            event_type: 3, // SyscallAnomaly
            severity: ThreatLevel::Medium as u8,
            _pad: [0; 2],
            pid: 0,
            detail1: syscall_rate_anomalies as u32,
            detail2: 0,
            timestamp: 0,
            correlation_id: 0,
            _pad2: 0,
        };
        inc_idx += 1;
    }

    report.incident_count = inc_idx as u8;

    // ── Recommendations ──
    let mut rec_idx = 0usize;

    // Kill processes involved in memory anomalies
    if mem_overflows > 0 || mem_uaf > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 0, // kill_process
                priority: 3, // immediate
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"kill_memory_anomaly_processes"),
            };
            rec_idx += 1;
        }
    }

    // Block IPs involved in port scans
    if net_scan_detections > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 1, // block_ip
                priority: 2, // high
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"block_port_scan_sources"),
            };
            rec_idx += 1;
        }
    }

    // Isolate processes involved in exfiltration
    if net_exfil_detections > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 2, // isolate
                priority: 3, // immediate
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"isolate_exfiltration_processes"),
            };
            rec_idx += 1;
        }
    }

    // Add IPC policy to block lateral movement
    if syscall_seq_matches > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 3, // add_policy
                priority: 2, // high
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"add_ipc_policy_block_lateral"),
            };
            rec_idx += 1;
        }
    }

    // Scan memory of affected processes
    if mem_overflows > 0 || mem_uaf > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 5, // scan_memory
                priority: 2, // high
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"scan_affected_memory"),
            };
            rec_idx += 1;
        }
    }

    // Review audit log
    if ipc_denied > 0 || exec_denied > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 6, // audit_review
                priority: 1, // medium
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"review_audit_log"),
            };
            rec_idx += 1;
        }
    }

    // Update signatures
    if mem_overflows > 0 || syscall_seq_matches > 0 {
        if rec_idx < MAX_RECOMMENDATIONS {
            report.recommendations[rec_idx] = Recommendation {
                rec_type: 7, // update_signatures
                priority: 1, // medium
                _pad: [0; 2],
                target_pid: 0,
                target_param: 0,
                description_hash: fnv1a_hash(b"update_threat_signatures"),
            };
            rec_idx += 1;
        }
    }

    report.recommendation_count = rec_idx as u8;

    // Compute checksum over the entire report (excluding the checksum field itself)
    let report_bytes = unsafe {
        core::slice::from_raw_parts(
            &report as *const Report as *const u8,
            core::mem::size_of::<Report>() - 4, // exclude the checksum field
        )
    };
    report.checksum = crc32(report_bytes);

    REPORTS_GENERATED.fetch_add(1, Ordering::Relaxed);
    LAST_REPORT_TIME.store(report.timestamp, Ordering::Release);

    report
}

/// Serializes a report into a binary format in a caller-provided buffer.
///
/// ## Binary format
/// - 16-byte header:
///   - Bytes 0–3:  magic number `0xF05E_F1LE`
///   - Bytes 4–7:  format version (u32 LE)
///   - Bytes 8–11: report size in bytes (u32 LE)
///   - Bytes 12–15: CRC-32 of the report payload
/// - Report payload: raw `#[repr(C)]` bytes of the `Report` struct
///
/// Returns the total number of bytes written.
pub fn serialize_report(report: &Report, buf: &mut [u8]) -> usize {
    let header_size = 16usize;
    let payload_size = core::mem::size_of::<Report>();
    let total_size = header_size + payload_size;

    if buf.len() < total_size {
        return 0;
    }

    // Write header
    buf[0..4].copy_from_slice(&REPORT_MAGIC_VALUE.to_le_bytes());
    buf[4..8].copy_from_slice(&REPORT_VERSION.to_le_bytes());
    buf[8..12].copy_from_slice(&(total_size as u32).to_le_bytes());
    buf[12..16].copy_from_slice(&report.checksum.to_le_bytes());

    // Write payload
    let payload_bytes = unsafe {
        core::slice::from_raw_parts(
            report as *const Report as *const u8,
            payload_size,
        )
    };
    buf[header_size..total_size].copy_from_slice(payload_bytes);

    total_size
}

/// Deserializes a report from a binary buffer.
///
/// Validates the magic number, version, and checksum.
/// Returns `Some(Report)` if valid, `None` otherwise.
pub fn deserialize_report(buf: &[u8]) -> Option<Report> {
    let header_size = 16usize;
    let payload_size = core::mem::size_of::<Report>();

    if buf.len() < header_size + payload_size {
        return None;
    }

    // Validate magic
    let magic = u32::from_le_bytes(buf[0..4].try_into().ok()?);
    if magic != REPORT_MAGIC_VALUE {
        return None;
    }

    // Validate version
    let version = u32::from_le_bytes(buf[4..8].try_into().ok()?);
    if version != REPORT_VERSION {
        return None;
    }

    // Read checksum
    let stored_checksum = u32::from_le_bytes(buf[12..16].try_into().ok()?);

    // Deserialize payload
    let mut report = Report::default();
    let payload_bytes = &buf[header_size..header_size + payload_size];
    let report_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut report as *mut Report as *mut u8,
            payload_size,
        )
    };
    report_bytes.copy_from_slice(payload_bytes);

    // Verify checksum (compute over the report excluding the checksum field)
    let check_bytes = unsafe {
        core::slice::from_raw_parts(
            &report as *const Report as *const u8,
            payload_size - 4,
        )
    };
    let computed_checksum = crc32(check_bytes);

    if computed_checksum != stored_checksum {
        return None;
    }

    Some(report)
}

/// Report subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ReportStats {
    pub reports_generated: u64,
    pub last_report_time: u64,
}

/// Collects report statistics.
pub fn get_report_stats() -> ReportStats {
    ReportStats {
        reports_generated: REPORTS_GENERATED.load(Ordering::Relaxed),
        last_report_time: LAST_REPORT_TIME.load(Ordering::Relaxed),
    }
}

/// Resets the report subsystem.
pub fn report_init() {
    REPORTS_GENERATED.store(0, Ordering::Release);
    LAST_REPORT_TIME.store(0, Ordering::Release);
}
