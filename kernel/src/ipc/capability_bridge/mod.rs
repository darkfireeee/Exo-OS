// kernel/src/ipc/capability_bridge/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAPABILITY BRIDGE — Shim léger IPC → security/capability/
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE C2 (DOC1) : capability/ JAMAIS dans ipc/ directement.
//   Ce bridge est un SHIM ~50 lignes qui ne fait que déléguer à security/capability/.
//   ZÉRO LOGIQUE ICI — wrapper d'appels uniquement.
//
// RAISONS :
//   • Périmètre de preuve Coq/TLA+ limité à security/capability/ (~500 lignes).
//   • ipc/ change d'API → fs/ ne casse pas (fs → security direct, pas ipc).
//   • TOCTOU impossible : une seule vérification dans security/capability/verify().
//
// CE MODULE RÉEXPORTE :
//   • check() → délègue à security::capability::verify()
//   • IpcCapBridge trait pour les objets qui vérifient les accès
// ═══════════════════════════════════════════════════════════════════════════════

pub mod check;

pub use check::{IpcCapBridge, verify_ipc_access, verify_endpoint_access};
