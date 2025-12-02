//! Security Audit Logging
//!
//! High-performance audit trail using lock-free ring buffer

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

/// Audit event type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditEventType {
    // Capability events
    CapabilityCreated = 0,
    CapabilityRevoked = 1,
    CapabilityTransferred = 2,
    CapabilityDerived = 3,

    // Permission events
    PermissionGranted = 10,
    PermissionDenied = 11,

    // Access events
    ObjectAccessed = 20,
    ObjectModified = 21,
    ObjectDeleted = 22,

    // Security events
    AuthenticationSuccess = 30,
    AuthenticationFailure = 31,
    PolicyViolation = 32,

    // TPM events
    TpmMeasurement = 40,
    TpmSeal = 41,
    TpmQuote = 42,
}

/// Audit event (compact, 32 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AuditEvent {
    /// Event ID (monotonic)
    pub id: u64,
    /// Timestamp (TSC or nanoseconds)
    pub timestamp: u64,
    /// Event type
    pub event_type: AuditEventType,
    /// Subject (who)
    pub subject: u32, // UID or PID
    /// Object (what)
    pub object: u64, // ObjectId
    /// Result (success/failure)
    pub success: bool,
    /// Additional data
    pub data: u64,
}

/// Audit log with ring buffer
pub struct AuditLog {
    buffer: Vec<AuditEvent>,
    write_pos: AtomicU64,
    next_id: AtomicU64,
    capacity: usize,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        let mut buffer = Vec::with_capacity(capacity);
        // Pre-allocate with zero events
        buffer.resize(
            capacity,
            AuditEvent {
                id: 0,
                timestamp: 0,
                event_type: AuditEventType::CapabilityCreated,
                subject: 0,
                object: 0,
                success: false,
                data: 0,
            },
        );

        Self {
            buffer,
            write_pos: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            capacity,
        }
    }

    /// Log an event (lock-free write)
    pub fn log(&self, event_type: AuditEventType, subject: u32, object: u64, success: bool) {
        let timestamp = self.get_timestamp();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let pos = self.write_pos.fetch_add(1, Ordering::Relaxed) as usize % self.capacity;

        let event = AuditEvent {
            id,
            timestamp,
            event_type,
            subject,
            object,
            success,
            data: 0,
        };

        // Write event (safe because we're the only writer for this position)
        unsafe {
            let ptr = self.buffer.as_ptr() as *mut AuditEvent;
            core::ptr::write_volatile(ptr.add(pos), event);
        }
    }

    /// Get timestamp (TSC for performance)
    fn get_timestamp(&self) -> u64 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::x86_64::_rdtsc()
        }

        #[cfg(not(target_arch = "x86_64"))]
        0
    }

    /// Get all events (snapshot)
    pub fn get_events(&self) -> Vec<AuditEvent> {
        self.buffer.clone()
    }

    /// Get events by type
    pub fn get_events_by_type(&self, event_type: AuditEventType) -> Vec<AuditEvent> {
        self.buffer
            .iter()
            .filter(|e| e.event_type == event_type && e.id > 0)
            .copied()
            .collect()
    }

    /// Get recent events (last N)
    pub fn get_recent(&self, count: usize) -> Vec<AuditEvent> {
        let pos = self.write_pos.load(Ordering::Acquire) as usize;
        let total = pos.min(self.capacity);
        let start = if total < count { 0 } else { total - count };

        self.buffer[start..total]
            .iter()
            .filter(|e| e.id > 0)
            .copied()
            .collect()
    }
}

/// Global audit log
static AUDIT_LOG: spin::Lazy<AuditLog> = spin::Lazy::new(|| AuditLog::new(16384)); // 16K events = 512KB

/// Log audit event globally
pub fn audit_log(event_type: AuditEventType, subject: u32, object: u64, success: bool) {
    if crate::security::config().audit_enabled {
        AUDIT_LOG.log(event_type, subject, object, success);
    }
}

/// Get audit summary
pub fn get_audit_summary() -> AuditSummary {
    let events = AUDIT_LOG.get_recent(1000);

    AuditSummary {
        total_events: events.len(),
        denied_count: events.iter().filter(|e| !e.success).count(),
        last_violation: events
            .iter()
            .rev()
            .find(|e| !e.success)
            .map(|e| e.timestamp),
    }
}

/// Get all events (for analysis)
pub fn get_all_events() -> Vec<AuditEvent> {
    AUDIT_LOG.get_events()
}

#[derive(Debug, Clone)]
pub struct AuditSummary {
    pub total_events: usize,
    pub denied_count: usize,
    pub last_violation: Option<u64>,
}
