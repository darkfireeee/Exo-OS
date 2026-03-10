// kernel/src/security/access_control/checker.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ACCESS CONTROL CHECKER — Point d'entrée unifié (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6 : remplace ipc/capability_bridge/ qui est supprimé.
//      Tous les modules (ipc/, fs/, process/, ...) appellent check_access()
//      au lieu d'accéder directement à capability::verify().
//
// RÈGLE SEC-AC-01 (v6) :
//   Tout accès à un objet protégé DOIT passer par check_access().
//   capability::verify() ne doit être appelé que depuis ce fichier.
//
// RÈGLE SEC-AC-02 :
//   check_access() logue TOUJOURS — succès et refus — via le module audit.
//
// FLUX DE VÉRIFICATION (v6) :
//   any_module
//     → security::access_control::check_access(table, token, kind, rights, caller)
//       → security::capability::verify::verify(table, token, rights)  [INV-1]
//       → security::audit  [log_event ou log_security_violation]
// ═══════════════════════════════════════════════════════════════════════════════


use crate::security::capability::verify::{verify as cap_verify, CapError};
use crate::security::capability::{CapTable, CapToken, Rights};
use crate::security::audit;
use crate::security::audit::{AuditCategory, AuditOutcome};
use super::object_types::ObjectKind;

// ─────────────────────────────────────────────────────────────────────────────
// AccessError — erreur riche retournée à l'appelant
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur d'accès retournée par `check_access()`.
/// Encapsule CapError avec contexte métier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// La vérification de capability a échoué.
    CapabilityDenied {
        reason: CapError,
        object: ObjectKind,
        module: &'static str,
    },
    /// L'objet demandé n'existe pas dans la table.
    ObjectNotFound {
        object: ObjectKind,
    },
    /// Les droits effectifs sont insuffisants.
    InsufficientRights {
        had:    Rights,
        needed: Rights,
    },
}

impl core::fmt::Display for AccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CapabilityDenied { reason, object, module } =>
                write!(f, "access_control: [{module}] denied for {object}: {reason}"),
            Self::ObjectNotFound { object } =>
                write!(f, "access_control: object not found: {object}"),
            Self::InsufficientRights { had: _, needed: _ } =>
                write!(f, "access_control: insufficient rights"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// check_access — POINT D'ENTRÉE UNIQUE (v6)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie l'accès à un objet protégé.
///
/// # Arguments
/// - `table`    : table de capabilities du processus appelant
/// - `token`    : capability token présentée
/// - `object`   : type d'objet demandé (pour audit et messages d'erreur)
/// - `required` : droits requis pour cette opération
/// - `caller`   : nom du module appelant ("ipc", "fs", "process", …)
///
/// # Comportement
/// - Appelle `capability::verify::verify()` (INV-1)
/// - Logue le résultat via `security::audit` (RÈGLE SEC-AC-02)
/// - Retourne `Ok(())` ou `Err(AccessError)`
///
/// # Performance
/// Même complexité que `verify()` : O(1).
/// Surcoût d'audit ≈ 1 écriture atomique dans le ring buffer.
#[inline]
pub fn check_access(
    table:    &CapTable,
    token:    CapToken,
    object:   ObjectKind,
    required: Rights,
    caller:   &'static str,
) -> Result<(), AccessError> {
    match cap_verify(table, token, required) {
        Ok(()) => {
            // ── Succès — log audit filtrable (catégorie Capability)
            audit::log_event(
                AuditCategory::Capability,
                0, 0, 0,
                0,
                0,
                AuditOutcome::Allow,
                [0u8; 8],
            );
            Ok(())
        }
        Err(CapError::ObjectNotFound) => {
            // Violation de sécurité — toujours loguée (RÈGLE AUDIT-02)
            audit::log_security_violation(0, 0, 0, 0, [0u8; 8]);
            Err(AccessError::ObjectNotFound { object })
        }
        Err(CapError::InsufficientRights) => {
            audit::audit_capability_deny(0, 0, 0, required.bits());
            Err(AccessError::InsufficientRights {
                had:    Rights::NONE,
                needed: required,
            })
        }
        Err(e) => {
            audit::audit_capability_deny(0, 0, 0, required.bits());
            Err(AccessError::CapabilityDenied {
                reason: e,
                object,
                module: caller,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// init
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le checker access_control.
///
/// Appelé par `security::security_init()` à l'étape 18 (v6 boot sequence).
/// Enregistre les mappings ObjectKind → droits attendus dans les règles d'audit.
pub fn init() {
    // Enregistrement des règles d'audit par type d'objet
    // (extensible : ajouter un add_global_rule() par ObjectKind si besoin)
    //
    // v6 : pas de table de dispatch runtime — les droits sont vérifiés
    // directement par cap_verify(). Cette fonction existe pour le hook
    // d'initialisation du boot (step 18) et pour la testabilité.
}
