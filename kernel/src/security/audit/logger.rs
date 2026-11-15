// kernel/src/security/audit/logger.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Audit Logger — Kernel audit log (syscalls, violations, events)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • Lock-free ring buffer of 65536 entries (1M memory)
//   • Each entry: 16 bytes minimum (category, tid, timestamp, data)
//   • Flush to userspace via shared memory interface (zero-copy)
//   • Distinct from tcb/audit.rs (TCB ring buffer = low-level security)
//     Here: high-level policy audit (LSM-like)
//
// RULE AUDIT-01: log_event() must be non-blocking (ISR-safe) — spin only.
// RULE AUDIT-02: Critical events (SECVIOL) cannot be filtered.
// RULE AUDIT-03: Buffer full -> oldest events overwritten (ring).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Event types
// ─────────────────────────────────────────────────────────────────────────────

/// Audit event category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditCategory {
    /// System call.
    Syscall = 0x01,
    /// Security policy violation.
    SecurityViolation = 0x02,
    /// Capability operation.
    Capability = 0x03,
    /// File system access.
    FileAccess = 0x04,
    /// Network operation.
    Network = 0x05,
    /// Process creation/termination.
    Process = 0x06,
    /// IPC operation.
    Ipc = 0x07,
    /// Authentication event.
    Auth = 0x08,
    /// Crypto violation (invalid signature, etc.).
    Crypto = 0x09,
    /// Boot event.
    Boot = 0x0A,
    /// Miscellaneous event.
    Other = 0xFF,
}

/// Operation outcome (for Syscall type events).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum AuditOutcome {
    Allow = 0,
    Deny = 1,
    Error = 2,
    Kill = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// Audit ring buffer entry
// ─────────────────────────────────────────────────────────────────────────────

/// Audit entry (16 bytes, cache-line friendly × 4 per cache line).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct AuditRecord {
    /// TSC timestamp (CPU cycles).
    pub timestamp: u64,
    /// Sender thread ID.
    pub thread_id: u32,
    /// Sender process ID.
    pub pid: u32,
    /// Syscall number (or 0).
    pub syscall_nr: u32,
    /// Error code / result.
    pub result: i32,
    /// Event category.
    pub category: AuditCategory,
    /// Operation outcome.
    pub outcome: AuditOutcome,
    /// Caller UID.
    pub uid: u16,
    /// Padding for 24 bytes total.
    pub _pad: [u8; 2],
    /// Additional data (path hash, addr, etc.).
    pub data: [u8; 8],
}

impl AuditRecord {
    pub const fn zeroed() -> Self {
        Self {
            timestamp: 0,
            thread_id: 0,
            pid: 0,
            syscall_nr: 0,
            result: 0,
            category: AuditCategory::Other,
            outcome: AuditOutcome::Allow,
            uid: 0,
            _pad: [0; 2],
            data: [0; 8],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ring buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Ring buffer size: 65536 entries.
const RING_SIZE: usize = 65536;

struct AuditRing {
    buffer: [AuditRecord; RING_SIZE],
    head: AtomicUsize, // Write position (producer)
    tail: AtomicUsize, // Read position (consumer)
    overflow: AtomicU64,
}

impl AuditRing {
    const fn new() -> Self {
        Self {
            buffer: [AuditRecord::zeroed(); RING_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    /// Writes an event into the ring buffer (RULE AUDIT-01: lock-free).
    ///
    /// Implements an OVERWRITE policy if buffer is full (RULE AUDIT-03).
    fn push(&mut self, record: AuditRecord) {
        let head_raw = self.head.fetch_add(1, Ordering::Relaxed);
        let idx = head_raw % RING_SIZE;
        // Check if unconsumed element will be overwritten using monotonic indices
        let tail = self.tail.load(Ordering::Relaxed);
        let used = head_raw.wrapping_sub(tail);
        if used >= RING_SIZE {
            // Buffer full: advance tail to preserve at most RING_SIZE - 1 unread entries
            self.tail.fetch_add(1, Ordering::Relaxed);
            self.overflow.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: idx < RING_SIZE by modulo construction
        unsafe {
            let ptr = self.buffer.as_mut_ptr().add(idx);
            core::ptr::write_volatile(ptr, record);
        }
    }

    /// Reads up to `max` entries from tail, returns number read.
    fn drain(&mut self, out: &mut [AuditRecord]) -> usize {
        let mut count = 0usize;
        while count < out.len() {
            let tail = self.tail.load(Ordering::Relaxed);
            let head_raw = self.head.load(Ordering::Relaxed);
            if head_raw == tail {
                break;
            }
            let idx = tail % RING_SIZE;
            // SAFETY: idx < RING_SIZE
            out[count] = unsafe { core::ptr::read_volatile(self.buffer.as_ptr().add(idx)) };
            self.tail.fetch_add(1, Ordering::Relaxed);
            count += 1;
        }
        count
    }

    fn pending(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail).min(RING_SIZE)
    }
}

static AUDIT_RING: spin::Mutex<AuditRing> = spin::Mutex::new(AuditRing::new());

// ─────────────────────────────────────────────────────────────────────────────
// Filters
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask of active categories (1 bit per AuditCategory u8).
/// Bit 1 = Syscall, bit 2 = SecurityViolation, etc.
static FILTER_MASK: AtomicU64 = AtomicU64::new(!0u64); // All enabled by default

static EVENTS_LOGGED: AtomicU64 = AtomicU64::new(0);
static EVENTS_DROPPED: AtomicU64 = AtomicU64::new(0);
static EVENTS_CRITICAL: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// TSC utility
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc available on x86_64; non-serialized sufficient for log timestamp.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
        );
    }
    (hi as u64) << 32 | lo as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Records an audit event.
///
/// RULE AUDIT-01: Non-blocking (short spin lock, no allocation).
/// RULE AUDIT-02: SecurityViolation cannot be filtered.
pub fn log_event(
    category: AuditCategory,
    pid: u32,
    thread_id: u32,
    uid: u16,
    syscall_nr: u32,
    result: i32,
    outcome: AuditOutcome,
    data: [u8; 8],
) {
    let critical = category == AuditCategory::SecurityViolation;

    // Check filter (RULE AUDIT-02: SecurityViolation always logged)
    let mask = FILTER_MASK.load(Ordering::Relaxed);
    let cat_bit = 1u64 << (category as u8);
    if !critical && (mask & cat_bit == 0) {
        EVENTS_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let record = AuditRecord {
        timestamp: rdtsc(),
        thread_id,
        pid,
        syscall_nr,
        result,
        category,
        outcome,
        uid,
        _pad: [0; 2],
        data,
    };

    let _irq = crate::scheduler::IrqGuard::new();
    AUDIT_RING.lock().push(record);
    EVENTS_LOGGED.fetch_add(1, Ordering::Relaxed);
    if critical {
        EVENTS_CRITICAL.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records a security violation (shortcut for RULE AUDIT-02).
pub fn log_security_violation(pid: u32, tid: u32, uid: u16, syscall_nr: u32, data: [u8; 8]) {
    log_event(
        AuditCategory::SecurityViolation,
        pid,
        tid,
        uid,
        syscall_nr,
        -1,
        AuditOutcome::Deny,
        data,
    );
}

/// Flushes pending events into `out`.
///
/// Returns number of copied events.
pub fn flush_to_userspace(out: &mut [AuditRecord]) -> usize {
    let _irq = crate::scheduler::IrqGuard::new();
    AUDIT_RING.lock().drain(out)
}

/// Number of events pending read.
pub fn pending_events() -> usize {
    let _irq = crate::scheduler::IrqGuard::new();
    AUDIT_RING.lock().pending()
}

/// Enables/disables an audit category (except SecurityViolation, always enabled).
pub fn set_filter(category: AuditCategory, enabled: bool) {
    if category == AuditCategory::SecurityViolation {
        return;
    } // RULE AUDIT-02
    let bit = 1u64 << (category as u8);
    if enabled {
        FILTER_MASK.fetch_or(bit, Ordering::Relaxed);
    } else {
        FILTER_MASK.fetch_and(!bit, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AuditLoggerStats {
    pub events_logged: u64,
    pub events_dropped: u64,
    pub events_critical: u64,
    pub overflow_count: u64,
    pub pending: usize,
}

pub fn audit_logger_stats() -> AuditLoggerStats {
    let _irq = crate::scheduler::IrqGuard::new();
    let ring = AUDIT_RING.lock();
    let overflow_count = ring.overflow.load(Ordering::Relaxed);
    let pending = ring.pending();
    drop(ring);
    AuditLoggerStats {
        events_logged: EVENTS_LOGGED.load(Ordering::Relaxed),
        events_dropped: EVENTS_DROPPED.load(Ordering::Relaxed),
        events_critical: EVENTS_CRITICAL.load(Ordering::Relaxed),
        overflow_count,
        pending,
    }
}
