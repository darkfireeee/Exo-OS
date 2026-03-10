// kernel/src/security/capability/revocation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// REVOCATION — Révocation de capabilities O(1) (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6 : Ce fichier ne contient QUE la révocation (revoke + revoke_token).
//      CapError et toutes les fonctions verify_*() ont été déplacées
//      dans verify.rs pour clarté architecturale.
//
// INVARIANT INV-2 (INVARIANTS.md) :
//   revoke(obj) → ∀ t avec t.generation == gen_avant,
//     verify(t, table_après) = Err(Revoked)
//   Couverture : proptest tests/invariants/revocation_proptest.rs
//
// COMPLEXITÉ : O(1) — fetch_add atomique Release
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::Ordering;

use super::token::{CapToken, ObjectId};
use super::table::CapTable;

// ─────────────────────────────────────────────────────────────────────────────
// revoke — Révocation O(1)
// ─────────────────────────────────────────────────────────────────────────────

/// Révoque TOUS les tokens existants pour un ObjectId.
///
/// Mécanisme : incrément atomique du compteur de génération dans la table.
/// Tous les tokens capturant l'ancienne génération retourneront Err(Revoked)
/// au prochain appel à `verify()`.
///
/// # Complexité : O(1)
/// Jamais de parcours des tokens existants (pas de liste de révocation).
///
/// # Sémantique mémoire
/// `Ordering::Release` : garantit que tous les accès précédant cette révocation
/// sont visibles AVANT que les lecteurs voient la nouvelle génération.
pub fn revoke(table: &CapTable, object_id: ObjectId) {
    table.increment_generation(object_id, Ordering::Release);
}

/// Révoque un token spécifique — wrapper autour de `revoke()`.
/// Note : révoque TOUS les tokens de l'objet référencé, pas uniquement celui fourni.
#[inline]
pub fn revoke_token(table: &CapTable, token: CapToken) {
    revoke(table, token.object_id());
}

