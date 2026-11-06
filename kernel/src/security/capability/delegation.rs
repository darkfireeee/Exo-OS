// kernel/src/security/capability/delegation.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// DELEGATION — Capability sub-delegation (Exo-OS Security · Layer 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  FORMAL VERIFICATION SCOPE — Any modification here REQUIRES updating
//     invariants in INVARIANTS.md + re-running proptest.
//
// RULE CAP-03 (verified by proptest + INVARIANTS.md — LAC-02):
//   delegated_rights.is_subset_of(source_rights) ALWAYS
//   It is FORBIDDEN to grant more rights than currently held.
//
// MODEL:
//   The delegator holds a source CapToken (must have DELEGATE right).
//   It transmits a subset of its rights to target table.
//   Target table receives a new CapToken referencing the same ObjectId.
//
// SPECIAL CASE — GRANT Delegation:
//   Only a token with GRANT right can allow target table
//   to regrant in turn. Without GRANT, the chain stops.
// ═══════════════════════════════════════════════════════════════════════════════

use super::rights::Rights;
use super::table::CapTable;
use super::token::{CapToken, ObjectId};
use super::verify::{verify, CapError};

// ─────────────────────────────────────────────────────────────────────────────
// Delegation functions
// ─────────────────────────────────────────────────────────────────────────────

/// Delegates a subset of rights from a source token to a target table.
///
/// # Preconditions
/// * `source_token` must be valid in `source_table`
/// * `source_token` must hold `Rights::DELEGATE` right
/// * `delegated_rights` must be a subset of `source_token` rights
///
/// # Returns
/// A new CapToken valid in `target_table` with `delegated_rights`.
///
/// # RULE CAP-03 (proptest + INVARIANTS.md — LAC-02)
/// delegated.is_subset_of(source.rights) — MANDATORILY verified here.
pub fn delegate(
    source_table: &CapTable,
    source_token: CapToken,
    target_table: &CapTable,
    delegated_rights: Rights,
) -> Result<CapToken, CapError> {
    // 1. Verify delegator has DELEGATE right
    verify(source_table, source_token, Rights::DELEGATE)?;

    // 2. RULE CAP-03: delegated rights CANNOT exceed source rights
    //    delegated_rights must be a subset of source_token.rights
    //    (without DELEGATE itself, unless explicitly included by caller)
    if !delegated_rights.is_subset_of(source_token.rights()) {
        return Err(CapError::InsufficientRights);
    }

    // 3. If delegated_rights contains GRANT -> verify source has GRANT too
    if delegated_rights.contains(Rights::GRANT) && !source_token.has_rights(Rights::GRANT) {
        return Err(CapError::InsufficientRights);
    }

    // 4. Grant new token in target table
    target_table.grant(
        source_token.object_id(),
        delegated_rights,
        source_token.object_type(),
    )
}

/// Delegates ALL rights from source token to target table (except REVOKE).
/// Shortcut for common case of cloning a capability.
pub fn delegate_all(
    source_table: &CapTable,
    source_token: CapToken,
    target_table: &CapTable,
) -> Result<CapToken, CapError> {
    // Delegate all rights except REVOKE — principle of least privilege.
    let delegated = source_token.rights() - Rights::REVOKE;
    delegate(source_table, source_token, target_table, delegated)
}

/// Delegates a read-only token — strips all write/control rights.
pub fn delegate_read_only(
    source_table: &CapTable,
    source_token: CapToken,
    target_table: &CapTable,
) -> Result<CapToken, CapError> {
    let read_only = Rights::READ & source_token.rights();
    if read_only.is_empty() {
        return Err(CapError::InsufficientRights);
    }
    delegate(source_table, source_token, target_table, read_only)
}

/// Verifies if a delegation is possible (without modifying target table).
/// Useful for pre-validation before a complex operation.
pub fn can_delegate(
    source_table: &CapTable,
    source_token: CapToken,
    delegated_rights: Rights,
) -> bool {
    // Fast check without modifying table
    if source_token.is_invalid() {
        return false;
    }
    if !source_token.has_rights(Rights::DELEGATE) {
        return false;
    }
    if !delegated_rights.is_subset_of(source_token.rights()) {
        return false;
    }
    // Verify generation (token not revoked)
    verify(source_table, source_token, Rights::DELEGATE).is_ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// DelegationChain — Delegation path audit
// ─────────────────────────────────────────────────────────────────────────────

/// Entry in a delegation chain.
#[derive(Debug, Clone, Copy)]
pub struct DelegationEntry {
    /// Shared object ObjectId.
    pub object_id: ObjectId,
    /// Rights granted at this step.
    pub rights: Rights,
    /// Depth in chain (0 = original owner).
    pub depth: u8,
}

/// Delegation chain — trace for audit log.
/// Fixed maximum capacity (no heap allocation on critical path).
pub struct DelegationChain {
    entries: [Option<DelegationEntry>; 8],
    len: usize,
}

impl DelegationChain {
    pub const MAX_DEPTH: usize = 8;

    pub fn new() -> Self {
        Self {
            entries: [None; 8],
            len: 0,
        }
    }

    /// Adds a step into the chain.
    /// Returns Err if maximum depth is exceeded.
    pub fn push(&mut self, entry: DelegationEntry) -> Result<(), CapError> {
        if self.len >= Self::MAX_DEPTH {
            return Err(CapError::DelegationDenied);
        }
        self.entries[self.len] = Some(entry);
        self.len += 1;
        Ok(())
    }

    /// Number of steps in chain.
    pub fn depth(&self) -> usize {
        self.len
    }

    /// Iterates over entries.
    pub fn iter(&self) -> impl Iterator<Item = &DelegationEntry> {
        self.entries[..self.len].iter().filter_map(|e| e.as_ref())
    }
}

impl Default for DelegationChain {
    fn default() -> Self {
        Self::new()
    }
}
