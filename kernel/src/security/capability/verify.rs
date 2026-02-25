// kernel/src/security/capability/verify.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// VERIFY — Point d'entrée UNIQUE de vérification de capabilities (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6 : extrait de revocation.rs pour clarté architecturale.
//      ipc/, fs/, process/ appellent TOUS cette fonction
//      via security::access_control::checker::check_access()
//
// RÈGLE SEC-01 (v6) :
//   security::capability::verify() est L'UNIQUE point de vérification dans tout l'OS.
//   Tout accès à un objet protégé DOIT passer par verify().
//   Appel direct hors de security/ = INTERDIT (passer par access_control/).
//
// INVARIANTS (INVARIANTS.md) :
//   INV-1 : verify(token, required) = Ok ⟹
//     token.object_id existe ∧ token.generation == table.generation ∧ rights.contains(required)
//   Couverture : proptest 1000+ cas (tests/invariants/capability_proptest.rs)
//
// COMPLEXITÉ : O(1) — lookup haché + 3 lectures atomiques
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use super::token::{CapToken, ObjectId, CapObjectType, stat_verified, stat_denied};
use super::rights::Rights;
use super::table::CapTable;

// ─────────────────────────────────────────────────────────────────────────────
// CapError — erreurs de vérification et de révocation
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur retournée par `verify()` et `revoke()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// L'objet référencé par le token n'existe pas dans la table.
    ObjectNotFound,
    /// Le token a été révoqué (génération incorrecte).
    Revoked,
    /// Les droits du token ne couvrent pas les droits requis.
    InsufficientRights,
    /// Le token est manifestement invalide (ObjectId::INVALID).
    InvalidToken,
    /// La table de capacités est pleine.
    TableFull,
    /// Tentative de délégation sans le droit DELEGATE.
    DelegationDenied,
    /// Violation d'invariant interne (ne devrait jamais se produire).
    InternalError,
}

impl core::fmt::Display for CapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ObjectNotFound      => write!(f, "capability: object not found"),
            Self::Revoked             => write!(f, "capability: token revoked"),
            Self::InsufficientRights  => write!(f, "capability: insufficient rights"),
            Self::InvalidToken        => write!(f, "capability: invalid token"),
            Self::TableFull           => write!(f, "capability: table full"),
            Self::DelegationDenied    => write!(f, "capability: delegation denied"),
            Self::InternalError       => write!(f, "capability: internal error"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// verify — POINT D'ENTRÉE UNIQUE de vérification (v6)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérification d'un CapToken — **UNIQUE point de vérification dans tout l'OS**.
///
/// # Algorithme (INV-1 — voir INVARIANTS.md)
/// 1. Rejet rapide : token invalide → Err(InvalidToken)
/// 2. Lookup O(1) dans la table par ObjectId haché
/// 3. Comparaison de génération → Err(Revoked) si différente (INV-2)
/// 4. Vérification des droits → Err(InsufficientRights) si insuffisants
/// 5. Ok(()) — accès autorisé
///
/// # Performance
/// Hot path typique : ~3 lectures atomiques + 2 comparaisons = ~10–15 cycles.
///
/// # Règle d'utilisation (v6)
/// Les modules ipc/, fs/, process/ ne doivent PAS appeler cette fonction
/// directement — passer par `security::access_control::checker::check_access()`
/// qui y ajoute le logging audit et un contexte d'erreur riche.
/// Seul `access_control/` appelle cette fonction.
#[inline]
pub fn verify(
    table:           &CapTable,
    token:           CapToken,
    required_rights: Rights,
) -> Result<(), CapError> {
    // 1. Rejet rapide des tokens manifestement invalides
    if token.is_invalid() {
        stat_denied();
        return Err(CapError::InvalidToken);
    }

    // 2. Lookup dans la table
    let entry = table.get(token.object_id()).ok_or_else(|| {
        stat_denied();
        CapError::ObjectNotFound
    })?;

    // 3. Vérification de génération — révocation O(1) (INV-2)
    if entry.generation != token.generation() {
        stat_denied();
        return Err(CapError::Revoked);
    }

    // 4. Vérification des droits (INV-3)
    if !entry.rights.contains(required_rights) {
        stat_denied();
        return Err(CapError::InsufficientRights);
    }

    stat_verified();
    Ok(())
}

/// Variante retournant les droits effectifs si succès.
/// Utilisée quand l'appelant a besoin de connaître les droits exacts.
#[inline]
pub fn verify_and_get_rights(
    table:           &CapTable,
    token:           CapToken,
    required_rights: Rights,
) -> Result<Rights, CapError> {
    verify(table, token, required_rights)?;
    let entry = table.get(token.object_id()).ok_or(CapError::ObjectNotFound)?;
    Ok(entry.rights)
}

/// Vérifie token ET type d'objet attendu en un seul appel.
#[inline]
pub fn verify_typed(
    table:         &CapTable,
    token:         CapToken,
    required:      Rights,
    expected_type: CapObjectType,
) -> Result<(), CapError> {
    verify(table, token, required)?;
    let entry = table.get(token.object_id()).ok_or(CapError::InternalError)?;
    if entry.type_tag != expected_type {
        stat_denied();
        return Err(CapError::InsufficientRights);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Raccourcis — hot paths fréquents
// ─────────────────────────────────────────────────────────────────────────────

/// Accès en lecture (hot path commun).
#[inline(always)]
pub fn verify_read(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::READ)
}

/// Accès en lecture + écriture.
#[inline(always)]
pub fn verify_read_write(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::READ_WRITE)
}

/// Envoi IPC.
#[inline(always)]
pub fn verify_ipc_send(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::IPC_SEND)
}

/// Réception IPC.
#[inline(always)]
pub fn verify_ipc_recv(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::IPC_RECV)
}
