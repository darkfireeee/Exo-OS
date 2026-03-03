//! audit/ — Journal d'audit ExoFS (no_std).
//!
//! Ring-buffer non-bloquant (65 536 entrées), écriture lock-free via
//! fetch_add atomique. Fournit : écriture, lecture, filtrage, rotation
//! et export des entrées.

pub mod audit_entry;
pub mod audit_log;
pub mod audit_writer;
pub mod audit_reader;
pub mod audit_rotation;
pub mod audit_filter;
pub mod audit_export;

pub use audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity, AuditSummary};
pub use audit_log::{AuditLog, AuditLogStats, AuditLogHealth, AUDIT_LOG};
pub use audit_writer::{AuditWriter, WriterContext, WritePolicy};
pub use audit_reader::{AuditReader, ReadDirection};
pub use audit_rotation::{AuditRotation, RotationConfig, RotationReport};
pub use audit_filter::{AuditFilter, FilterCriteria, FilterChain};
pub use audit_export::{AuditExporter, ExportFormat, ExportRange};

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de cycle de vie
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le module audit (idempotent).
///
/// Le ring-buffer est un `static` initialisé en `const` — aucune allocation
/// n'est nécessaire. Cette fonction est conservée pour indiquer l'ordre
/// d'initialisation dans `exofs::init()`.
pub fn init() {
    // AUDIT_LOG est un static avec new_const() — rien à allouer.
}

/// Remet les compteurs statistiques du ring à zéro.
///
/// À appeler lors d'une rotation ou d'un démontage propre.
pub fn reset_stats() {
    AUDIT_LOG.reset_stats();
}

/// Vérifie la sanité du ring-buffer (vérifie N entrées récentes).
///
/// Retourne `true` si aucune entrée corrompue dans l'échantillon.
pub fn verify_health() -> bool {
    let health = AUDIT_LOG.sanity_check(64);
    health.is_clean()
}

/// Retourne `true` si le ring dépasse 80 % de remplissage.
///
/// Le module appelant devrait déclencher une rotation.
pub fn needs_rotation() -> bool {
    AUDIT_LOG.stats().is_near_full()
}

/// Écrit un événement de refus de permission depuis n'importe quel module.
///
/// Raccourci pour éviter d'instancier un `AuditWriter` complet.
pub fn perm_denied(actor_uid: u64, object_id: u64, op: AuditOp) {
    audit_writer::record_perm_denied(actor_uid, object_id, op);
}

