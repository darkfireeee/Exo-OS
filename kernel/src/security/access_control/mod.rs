// kernel/src/security/access_control/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ACCESS CONTROL — Module racine (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6 : remplace ipc/capability_bridge/ (supprimé).
//      Point d'entrée unifié pour TOUS les modules (ipc/, fs/, process/).
//
// Structure :
//   access_control/
//   ├── checker.rs      — check_access() point d'entrée unique + init()
//   └── object_types.rs — ObjectKind enum + droits associés
//
// RÈGLE SEC-AC-01 (v6) :
//   Tout accès à un objet protégé DOIT passer par checker::check_access().
//   Direct calls à security::capability::verify() depuis ipc/fs/process = INTERDIT.
//
// FLUX (v6) :
//   ipc/ | fs/ | process/
//     → access_control::check_access(table, token, ObjectKind::X, rights, "module")
//       → capability::verify::verify(table, token, rights)   [O(1), INV-1]
//       → audit::log_event / audit_capability_deny
// ═══════════════════════════════════════════════════════════════════════════════

pub mod checker;
pub mod object_types;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

pub use checker::{check_access, init, AccessError};
pub use object_types::ObjectKind;
