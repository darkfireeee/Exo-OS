// kernel/src/security/capability/verify.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// VERIFY — SINGLE entry point for capability verification (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6: extracted from revocation.rs for architectural clarity.
//     ipc/, fs/, process/ ALL call this function
//     via security::access_control::checker::check_access()
//
// RULE SEC-01 (v6):
//   security::capability::verify() is the SINGLE verification point across entire OS.
//   Any access to a protected object MUST go through verify().
//   Direct call outside security/ = FORBIDDEN (go through access_control/).
//
// INVARIANTS (INVARIANTS.md):
//   INV-1: verify(token, required) = Ok ⟹
//     token.object_id exists ∧ token.generation == table.generation ∧ rights.contains(required)
//   Coverage: proptest 1000+ cases (tests/invariants/capability_proptest.rs)
//
// COMPLEXITY: O(1) — hashed lookup + 3 atomic reads
// ═══════════════════════════════════════════════════════════════════════════════

use super::rights::Rights;
use super::table::CapTable;
use super::token::{stat_denied, stat_verified, CapObjectType, CapToken};
use subtle::{Choice, ConstantTimeEq};

// ─────────────────────────────────────────────────────────────────────────────
// CapError — Verification and revocation errors
// ─────────────────────────────────────────────────────────────────────────────

/// Error returned by `verify()` and `revoke()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// Object referenced by token does not exist in table.
    ObjectNotFound,
    /// Token has been revoked (incorrect generation).
    Revoked,
    /// Token rights do not cover required rights.
    InsufficientRights,
    /// Token is manifestly invalid (ObjectId::INVALID).
    InvalidToken,
    /// Capability table is full.
    TableFull,
    /// Attempted delegation without DELEGATE right.
    DelegationDenied,
    /// Internal invariant violation (should never happen).
    InternalError,
    /// Generic constant-time denial (CAP-05): verify() ALWAYS returns
    /// this variant upon failure — never ObjectNotFound nor Revoked.
    /// Prevents enumeration of valid ObjectIds via timing side-channel.
    Denied,
}

impl core::fmt::Display for CapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ObjectNotFound => write!(f, "capability: object not found"),
            Self::Revoked => write!(f, "capability: token revoked"),
            Self::InsufficientRights => write!(f, "capability: insufficient rights"),
            Self::InvalidToken => write!(f, "capability: invalid token"),
            Self::TableFull => write!(f, "capability: table full"),
            Self::DelegationDenied => write!(f, "capability: delegation denied"),
            Self::InternalError => write!(f, "capability: internal error"),
            Self::Denied => write!(f, "capability: denied"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// verify — SINGLE verification entry point (v6)
// ─────────────────────────────────────────────────────────────────────────────

/// Verification of a CapToken — **SINGLE verification point across entire OS**.
///
/// # Algorithm (INV-1 — see INVARIANTS.md)
/// 1. Fast reject: invalid token -> Err(InvalidToken)
/// 2. Hashed lookup O(1) in table by ObjectId
/// 3. Generation comparison -> Err(Revoked) if different (INV-2)
/// 4. Rights verification -> Err(InsufficientRights) if insufficient
/// 5. Ok(()) — access authorized
///
/// # Performance
/// Typical hot path: ~3 atomic reads + 2 comparisons = ~10–15 cycles.
///
/// # Usage rule (v6)
/// Modules ipc/, fs/, process/ must NOT call this function
/// directly — go through `security::access_control::checker::check_access()`
/// which adds audit logging and rich error context.
/// Only `access_control/` calls this function.
/// CapToken verification — **SINGLE verification point across entire OS**.
///
/// # Property CAP-05 — Constant-time (see INVARIANTS.md)
/// verify() ALWAYS executes the same sequence of operations regardless of
/// outcome. No differential early return between "missing object" and "revoked".
/// ALWAYS returns `Err(Denied)` if anything fails — never
/// `ObjectNotFound` nor `Revoked` — to prevent enumeration of valid ObjectIds
/// via timing measurements by an attacker.
///
/// # Algorithm (INV-1 — see INVARIANTS.md)
/// 1. Immediate rejection of INVALID token (reveals nothing about table).
/// 2. Hashed lookup O(1) in table by ObjectId — identical path if absent.
/// 3. Generation + rights comparisons performed EVEN IF entry absent
///    (sentinel values u32::MAX / Rights::empty()).
/// 4. Combined outcome — single error code Denied.
///
/// # Usage rule (v6)
/// Modules ipc/, fs/, process/ must NOT call this function
/// directly — go through `security::access_control::checker::check_access()`
/// which adds audit logging and rich error context.
#[inline]
pub fn verify(table: &CapTable, token: CapToken, required_rights: Rights) -> Result<(), CapError> {
    // 1. "invalid token" case follows same path as other
    //    denials to equalize timing (CAP-05).
    let token_invalid = token.is_invalid();

    // 2. Lookup — identical path whether entry found or not.
    let entry_opt = table.get(token.object_id());

    // 3. Sentinel values: same comparisons if entry is absent.
    //    u32::MAX can never be a valid generation (monotonic counter).
    //    Rights::empty() will never satisfy contains(required) if required != 0.
    let stored_gen = entry_opt.as_ref().map(|e| e.generation).unwrap_or(u32::MAX);
    let stored_rights = entry_opt
        .as_ref()
        .map(|e| e.rights)
        .unwrap_or(Rights::empty());
    let entry_found = Choice::from(entry_opt.is_some() as u8);

    // 4. Both comparisons are ALWAYS performed — no short-circuit evaluation.
    let gen_ok = stored_gen.ct_eq(&token.generation());
    let rights_ok = stored_rights.contains_ct(required_rights);
    let access_ok = entry_found & gen_ok & rights_ok;

    // 5. Unified result — Denied in ALL failure cases (CAP-05).
    if token_invalid | !bool::from(access_ok) {
        stat_denied();
        return Err(CapError::Denied);
    }

    stat_verified();
    Ok(())
}

/// Variant returning effective rights on success.
/// Used when caller needs to know exact rights.
#[inline]
pub fn verify_and_get_rights(
    table: &CapTable,
    token: CapToken,
    required_rights: Rights,
) -> Result<Rights, CapError> {
    verify(table, token, required_rights)?;
    let entry = table
        .get(token.object_id())
        .ok_or(CapError::ObjectNotFound)?;
    Ok(entry.rights)
}

/// Verifies token AND expected object type in a single call.
#[inline]
pub fn verify_typed(
    table: &CapTable,
    token: CapToken,
    required: Rights,
    expected_type: CapObjectType,
) -> Result<(), CapError> {
    verify(table, token, required)?;
    let entry = table
        .get(token.object_id())
        .ok_or(CapError::InternalError)?;
    if entry.type_tag != expected_type {
        stat_denied();
        return Err(CapError::InsufficientRights);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Shortcuts — Frequent hot paths
// ─────────────────────────────────────────────────────────────────────────────

/// Read access (common hot path).
#[inline(always)]
pub fn verify_read(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::READ)
}

/// Read + write access.
#[inline(always)]
pub fn verify_read_write(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::READ_WRITE)
}

/// IPC send.
#[inline(always)]
pub fn verify_ipc_send(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::IPC_SEND)
}

/// IPC receive.
#[inline(always)]
pub fn verify_ipc_recv(table: &CapTable, token: CapToken) -> Result<(), CapError> {
    verify(table, token, Rights::IPC_RECV)
}
