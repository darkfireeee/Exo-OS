// kernel/src/security/capability/revocation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// REVOCATION — O(1) capability revocation (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6: This file contains ONLY revocation (revoke + revoke_token).
//     CapError and all verify_*() functions were moved
//     into verify.rs for architectural clarity.
//
// INVARIANT INV-2 (INVARIANTS.md):
//   revoke(obj) -> ∀ t with t.generation == gen_before,
//     verify(t, table_after) = Err(Revoked)
//   Coverage: proptest tests/invariants/revocation_proptest.rs
//
// COMPLEXITY: O(1) — atomic fetch_add Release
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::Ordering;

use super::table::CapTable;
use super::token::{CapToken, ObjectId};

// ─────────────────────────────────────────────────────────────────────────────
// revoke — O(1) revocation
// ─────────────────────────────────────────────────────────────────────────────

/// Revokes ALL existing tokens for an ObjectId.
///
/// Mechanism: atomic increment of generation counter in table.
/// All tokens capturing the previous generation will return Err(Revoked)
/// on the next call to `verify()`.
///
/// # Complexity: O(1)
/// Never traverses existing tokens (no revocation list).
///
/// # Memory semantics
/// `Ordering::Release`: guarantees all accesses preceding this revocation
/// are visible BEFORE readers see the new generation.
pub fn revoke(table: &CapTable, object_id: ObjectId) {
    table.increment_generation(object_id, Ordering::Release);
}

/// Revokes a specific token — wrapper around `revoke()`.
/// Note: revokes ALL tokens of referenced object, not only the one provided.
#[inline]
pub fn revoke_token(table: &CapTable, token: CapToken) {
    revoke(table, token.object_id());
}
