//! audit/ — Journal d'audit ExoFS (no_std).
//! Ring-buffer non-bloquant, jamais de perte d'événement.

pub mod audit_entry;
pub mod audit_log;
pub mod audit_writer;
pub mod audit_reader;
pub mod audit_rotation;
pub mod audit_filter;
pub mod audit_export;

pub use audit_entry::{AuditEntry, AuditOp, AuditResult};
pub use audit_log::{AuditLog, AUDIT_LOG};
pub use audit_writer::AuditWriter;
pub use audit_reader::AuditReader;
pub use audit_rotation::{AuditRotation, RotationConfig};
pub use audit_filter::{AuditFilter, FilterCriteria};
pub use audit_export::AuditExporter;
