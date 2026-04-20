//! # audit — IPC audit logging ring buffer
//!
//! Maintains a fixed-size ring buffer of IPC audit entries. Provides
//! query, filter, and export capabilities for forensic analysis.
//!
//! ## Design
//! - Ring buffer of 1024 entries (oldest entries are overwritten)
//! - Each entry records source PID, destination PID, message type,
//!   policy action, and a timestamp
//! - Query/filter by PID pair, message type, action, or time range
//! - Export entries to a caller-provided byte buffer in a compact
//!   binary format for persistent storage

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of audit entries in the ring buffer.
const MAX_AUDIT_ENTRIES: usize = 1024;

/// Maximum export buffer size for binary serialization.
const MAX_EXPORT_SIZE: usize = 8192;

// ── Types ─────────────────────────────────────────────────────────────────────

/// A single IPC audit entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuditEntry {
    /// Source PID of the IPC message.
    pub src_pid: u32,
    /// Destination PID of the IPC message.
    pub dst_pid: u32,
    /// IPC message type.
    pub msg_type: u32,
    /// Policy action taken: 0=Allow, 1=Deny, 2=AuditOnly, 3=RateLimit.
    pub action: u8,
    /// Rule ID that was matched (0 = default policy).
    pub rule_id: u32,
    /// Reply nonce from the IPC message.
    pub reply_nonce: u32,
    /// Result code: 0=success, 1=blocked, 2=rate_limited.
    pub result: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// TSC timestamp of the audit event.
    pub timestamp: u64,
}

impl Default for AuditEntry {
    fn default() -> Self {
        Self {
            src_pid: 0,
            dst_pid: 0,
            msg_type: 0,
            action: 0,
            rule_id: 0,
            reply_nonce: 0,
            result: 0,
            _pad: [0; 2],
            timestamp: 0,
        }
    }
}

// Compile-time size assertion — AuditEntry should be 36 bytes.
const _: () = assert!(
    core::mem::size_of::<AuditEntry>() == 36,
    "AuditEntry must be 36 bytes for ABI compatibility"
);

/// Result of an audit query.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuditResult {
    /// Number of entries matched.
    pub matched: u32,
    /// Number of entries written to the output buffer.
    pub written: u32,
}

impl Default for AuditResult {
    fn default() -> Self {
        Self { matched: 0, written: 0 }
    }
}

/// Filter criteria for audit queries.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuditFilter {
    /// Filter by source PID (0 = no filter).
    pub src_pid: u32,
    /// Filter by destination PID (0 = no filter).
    pub dst_pid: u32,
    /// Filter by message type (0 = no filter).
    pub msg_type: u32,
    /// Filter by action (0xFF = no filter).
    pub action: u8,
    /// Filter by result code (0xFF = no filter).
    pub result: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// Minimum timestamp (0 = no minimum).
    pub min_timestamp: u64,
    /// Maximum timestamp (0 = no maximum).
    pub max_timestamp: u64,
}

impl Default for AuditFilter {
    fn default() -> Self {
        Self {
            src_pid: 0,
            dst_pid: 0,
            msg_type: 0,
            action: 0xFF,
            result: 0xFF,
            _pad: [0; 2],
            min_timestamp: 0,
            max_timestamp: 0,
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

/// Audit ring buffer.
static AUDIT_BUFFER: Mutex<[AuditEntry; MAX_AUDIT_ENTRIES]> = Mutex::new(
    [AuditEntry::default(); MAX_AUDIT_ENTRIES],
);

/// Write index into the audit ring buffer.
static AUDIT_WRITE_IDX: AtomicU32 = AtomicU32::new(0);

/// Total number of entries ever written (for sequence numbering).
static AUDIT_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Statistics.
static AUDIT_ALLOWED: AtomicU64 = AtomicU64::new(0);
static AUDIT_DENIED: AtomicU64 = AtomicU64::new(0);
static AUDIT_RATE_LIMITED: AtomicU64 = AtomicU64::new(0);
static AUDIT_QUERIES: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Check if an entry matches the filter criteria.
fn entry_matches_filter(entry: &AuditEntry, filter: &AuditFilter) -> bool {
    // Source PID filter
    if filter.src_pid != 0 && entry.src_pid != filter.src_pid {
        return false;
    }

    // Destination PID filter
    if filter.dst_pid != 0 && entry.dst_pid != filter.dst_pid {
        return false;
    }

    // Message type filter
    if filter.msg_type != 0 && entry.msg_type != filter.msg_type {
        return false;
    }

    // Action filter
    if filter.action != 0xFF && entry.action != filter.action {
        return false;
    }

    // Result filter
    if filter.result != 0xFF && entry.result != filter.result {
        return false;
    }

    // Minimum timestamp
    if filter.min_timestamp != 0 && entry.timestamp < filter.min_timestamp {
        return false;
    }

    // Maximum timestamp
    if filter.max_timestamp != 0 && entry.timestamp > filter.max_timestamp {
        return false;
    }

    true
}

// ── Ring buffer abstraction ───────────────────────────────────────────────────

/// Audit ring buffer handle — provides typed access to the static buffer.
pub struct AuditRingBuffer;

impl AuditRingBuffer {
    /// Creates a new handle to the global audit ring buffer.
    pub const fn new() -> Self {
        Self
    }

    /// Writes an entry to the ring buffer.
    pub fn write(&self, entry: AuditEntry) {
        record_audit(entry);
    }

    /// Reads the most recent `count` entries into `out`.
    /// Returns the number of entries actually written.
    pub fn read_recent(&self, out: &mut [AuditEntry], count: usize) -> usize {
        let n = count.min(out.len());
        let buffer = AUDIT_BUFFER.lock();
        let head = AUDIT_WRITE_IDX.load(Ordering::Acquire) as usize;
        let mut written = 0usize;

        for offset in 0..n.min(MAX_AUDIT_ENTRIES) {
            let i = (head + MAX_AUDIT_ENTRIES - 1 - offset) % MAX_AUDIT_ENTRIES;
            if buffer[i].timestamp != 0 {
                out[written] = buffer[i];
                written += 1;
            }
        }
        written
    }

    /// Returns the total number of entries ever written.
    pub fn total_entries(&self) -> u64 {
        AUDIT_TOTAL.load(Ordering::Relaxed)
    }

    /// Returns the current number of valid entries in the buffer.
    pub fn len(&self) -> usize {
        let total = AUDIT_TOTAL.load(Ordering::Relaxed) as usize;
        total.min(MAX_AUDIT_ENTRIES)
    }

    /// Returns whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        AUDIT_TOTAL.load(Ordering::Relaxed) == 0
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Records an audit entry into the ring buffer.
///
/// The entry is written at the current write position, which wraps
/// around when the buffer is full (overwriting the oldest entry).
pub fn record_audit(entry: AuditEntry) {
    let mut buffer = AUDIT_BUFFER.lock();
    let idx = AUDIT_WRITE_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_AUDIT_ENTRIES;
    buffer[idx] = entry;
    AUDIT_TOTAL.fetch_add(1, Ordering::Relaxed);

    // Update action-specific counters
    match entry.action {
        0 => AUDIT_ALLOWED.fetch_add(1, Ordering::Relaxed),
        1 => AUDIT_DENIED.fetch_add(1, Ordering::Relaxed),
        3 => AUDIT_RATE_LIMITED.fetch_add(1, Ordering::Relaxed),
        _ => {}
    }
}

/// Queries the audit log for entries matching the given filter.
///
/// Scans the ring buffer from most recent to oldest. Returns up to
/// `out.len()` matching entries and an `AuditResult` with total match
/// count.
pub fn query_audit_filtered(filter: &AuditFilter, out: &mut [AuditEntry]) -> AuditResult {
    AUDIT_QUERIES.fetch_add(1, Ordering::Relaxed);

    let buffer = AUDIT_BUFFER.lock();
    let head = AUDIT_WRITE_IDX.load(Ordering::Acquire) as usize;
    let mut matched = 0u32;
    let mut written = 0u32;

    for offset in 0..MAX_AUDIT_ENTRIES {
        let i = (head + MAX_AUDIT_ENTRIES - 1 - offset) % MAX_AUDIT_ENTRIES;
        if buffer[i].timestamp == 0 {
            continue;
        }
        if entry_matches_filter(&buffer[i], filter) {
            matched += 1;
            if (written as usize) < out.len() {
                out[written as usize] = buffer[i];
                written += 1;
            }
        }
    }

    AuditResult { matched, written }
}

/// Queries the audit log for all entries involving a specific PID
/// (either as source or destination).
///
/// Convenience wrapper around `query_audit_filtered`.
pub fn query_audit(pid: u32, out: &mut [AuditEntry]) -> AuditResult {
    // Query as source
    let filter_src = AuditFilter {
        src_pid: pid,
        ..AuditFilter::default()
    };
    let mut result = query_audit_filtered(&filter_src, out);

    // Query as destination (append to out buffer)
    let filter_dst = AuditFilter {
        dst_pid: pid,
        ..AuditFilter::default()
    };
    let offset = result.written as usize;
    if offset < out.len() {
        let remaining = &mut out[offset..];
        let dst_result = query_audit_filtered(&filter_dst, remaining);
        result.matched += dst_result.matched;
        result.written += dst_result.written;
    }

    result
}

/// Exports audit entries to a binary format in a caller-provided buffer.
///
/// ## Binary format
/// Each entry is serialized as its raw 36-byte `#[repr(C)]` representation.
/// A 16-byte header is prepended:
/// - Bytes 0–3:  magic number `0xE5A1D700` (little-endian)
/// - Bytes 4–7:  number of entries (u32 LE)
/// - Bytes 8–11: entry size in bytes (u32 LE = 36)
/// - Bytes 12–15: reserved (0)
///
/// Returns the total number of bytes written to the buffer.
pub fn export_audit(filter: &AuditFilter, buf: &mut [u8]) -> usize {
    AUDIT_QUERIES.fetch_add(1, Ordering::Relaxed);

    if buf.len() < 16 {
        return 0;
    }

    // Write header
    let magic: u32 = 0xE5A1_D700;
    buf[0..4].copy_from_slice(&magic.to_le_bytes());
    // Entry count and size will be filled after counting
    buf[8..12].copy_from_slice(&36u32.to_le_bytes());
    buf[12..16].copy_from_slice(&0u32.to_le_bytes());

    let buffer = AUDIT_BUFFER.lock();
    let head = AUDIT_WRITE_IDX.load(Ordering::Acquire) as usize;
    let mut entry_count = 0u32;
    let mut write_offset = 16usize;

    for offset in 0..MAX_AUDIT_ENTRIES {
        if write_offset + 36 > buf.len() {
            break;
        }
        let i = (head + MAX_AUDIT_ENTRIES - 1 - offset) % MAX_AUDIT_ENTRIES;
        if buffer[i].timestamp == 0 {
            continue;
        }
        if entry_matches_filter(&buffer[i], filter) {
            // Serialize the entry as raw bytes
            let entry_bytes = unsafe {
                core::slice::from_raw_parts(
                    &buffer[i] as *const AuditEntry as *const u8,
                    core::mem::size_of::<AuditEntry>(),
                )
            };
            buf[write_offset..write_offset + 36].copy_from_slice(entry_bytes);
            write_offset += 36;
            entry_count += 1;
        }
    }

    // Update entry count in header
    buf[4..8].copy_from_slice(&entry_count.to_le_bytes());

    write_offset
}

/// Audit subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AuditStats {
    pub total_entries: u64,
    pub allowed: u64,
    pub denied: u64,
    pub rate_limited: u64,
    pub queries: u64,
    pub buffer_usage: u32,
}

/// Collects audit statistics.
pub fn get_audit_stats() -> AuditStats {
    let total = AUDIT_TOTAL.load(Ordering::Relaxed);
    AuditStats {
        total_entries: total,
        allowed: AUDIT_ALLOWED.load(Ordering::Relaxed),
        denied: AUDIT_DENIED.load(Ordering::Relaxed),
        rate_limited: AUDIT_RATE_LIMITED.load(Ordering::Relaxed),
        queries: AUDIT_QUERIES.load(Ordering::Relaxed),
        buffer_usage: (total as u32).min(MAX_AUDIT_ENTRIES as u32),
    }
}

/// Queries audit entries by time range.
///
/// Returns entries with timestamps in `[min_ts, max_ts]`.
pub fn query_audit_by_time(min_ts: u64, max_ts: u64, out: &mut [AuditEntry]) -> AuditResult {
    let filter = AuditFilter {
        min_timestamp: min_ts,
        max_timestamp: max_ts,
        ..AuditFilter::default()
    };
    query_audit_filtered(&filter, out)
}

/// Queries audit entries for a specific (src_pid, dst_pid) pair.
pub fn query_audit_pair(src_pid: u32, dst_pid: u32, out: &mut [AuditEntry]) -> AuditResult {
    let filter = AuditFilter {
        src_pid,
        dst_pid,
        ..AuditFilter::default()
    };
    query_audit_filtered(&filter, out)
}

/// Resets the audit subsystem.
pub fn audit_init() {
    AUDIT_WRITE_IDX.store(0, Ordering::Release);
    AUDIT_TOTAL.store(0, Ordering::Release);
    AUDIT_ALLOWED.store(0, Ordering::Release);
    AUDIT_DENIED.store(0, Ordering::Release);
    AUDIT_RATE_LIMITED.store(0, Ordering::Release);
    AUDIT_QUERIES.store(0, Ordering::Release);
}
