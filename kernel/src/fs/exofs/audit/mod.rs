//! audit/ — ExoFS audit journal (no_std).
//!
//! Non-blocking ring-buffer (65,536 entries), lock-free write via
//! atomic fetch_add. Provides: writing, reading, filtering, rotation,
//! and export of entries.

pub mod audit_entry;
pub mod audit_export;
pub mod audit_filter;
pub mod audit_log;
pub mod audit_reader;
pub mod audit_rotation;
pub mod audit_writer;

pub use audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity, AuditSummary};
pub use audit_export::{AuditExporter, ExportFormat, ExportRange};
pub use audit_filter::{AuditFilter, FilterChain, FilterCriteria};
pub use audit_log::{AuditLog, AuditLogHealth, AuditLogStats, AUDIT_LOG};
pub use audit_reader::{AuditReader, ReadDirection};
pub use audit_rotation::{AuditRotation, RotationConfig, RotationReport};
pub use audit_writer::{AuditWriter, WritePolicy, WriterContext};

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle functions
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the audit module (idempotent).
///
/// The ring-buffer is a `static` initialized with `const` — no allocation
/// is needed. This function is kept to indicate initialization order
/// in `exofs::init()`.
pub fn init() {
    // AUDIT_LOG is a static with new_const() — nothing to allocate.
}

/// Resets ring statistical counters to zero.
///
/// To be called upon rotation or clean unmount.
pub fn reset_stats() {
    AUDIT_LOG.reset_stats();
}

/// Verifies ring-buffer sanity (checks N recent entries).
///
/// Returns `true` if no corrupt entries found in sample.
pub fn verify_health() -> bool {
    let health = AUDIT_LOG.sanity_check(64);
    health.is_clean()
}

/// Returns `true` if ring exceeds 80% capacity.
///
/// Calling module should trigger a rotation.
pub fn needs_rotation() -> bool {
    AUDIT_LOG.stats().is_near_full()
}

/// Writes a permission denial event from any module.
///
/// Shortcut to avoid instantiating a full `AuditWriter`.
pub fn perm_denied(actor_uid: u64, object_id: u64, op: AuditOp) {
    audit_writer::record_perm_denied(actor_uid, object_id, op);
}
