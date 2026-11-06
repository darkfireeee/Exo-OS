// kernel/src/security/access_control/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ACCESS CONTROL — Root module (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6: replaces ipc/capability_bridge/ (removed).
//     Unified entry point for ALL modules (ipc/, fs/, process/).
//
// Structure:
//   access_control/
//   ├── checker.rs      — check_access() single entry point + init()
//   └── object_types.rs — ObjectKind enum + associated rights
//
// RULE SEC-AC-01 (v6):
//   Any access to a protected object MUST go through checker::check_access().
//   Direct calls to security::capability::verify() from ipc/fs/process = FORBIDDEN.
//
// FLOW (v6):
//   ipc/ | fs/ | process/
//     -> access_control::check_access(table, token, ObjectKind::X, rights, "module")
//       -> capability::verify::verify(table, token, rights)   [O(1), INV-1]
//       -> audit::log_event / audit_capability_deny
// ═══════════════════════════════════════════════════════════════════════════════

pub mod checker;
pub mod object_types;

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use checker::{check_access, init, AccessError};
pub use object_types::ObjectKind;
