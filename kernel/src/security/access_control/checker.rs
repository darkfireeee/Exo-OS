// kernel/src/security/access_control/checker.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ACCESS CONTROL CHECKER — Unified entry point (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// v6: replaces ipc/capability_bridge/ which is removed.
//     All modules (ipc/, fs/, process/, ...) call check_access()
//     instead of directly accessing capability::verify().
//
// RULE SEC-AC-01 (v6):
//   Any access to a protected object MUST go through check_access().
//   capability::verify() must only be called from this file.
//
// RULE SEC-AC-02:
//   check_access() ALWAYS logs — success and denial — via the audit module.
//
// VERIFICATION FLOW (v6):
//   any_module
//     -> security::access_control::check_access(table, token, kind, rights, caller)
//       -> security::capability::verify::verify(table, token, rights)  [INV-1]
//       -> security::audit  [log_event or log_security_violation]
// ═══════════════════════════════════════════════════════════════════════════════

use super::object_types::ObjectKind;
use crate::security::audit;
use crate::security::audit::{AuditCategory, AuditOutcome};
use crate::security::capability::verify::{verify as cap_verify, CapError};
use crate::security::capability::{CapTable, CapToken, Rights};

// ─────────────────────────────────────────────────────────────────────────────
// AccessError — Rich error returned to caller
// ─────────────────────────────────────────────────────────────────────────────

/// Access error returned by `check_access()`.
/// Encapsulates CapError with domain context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// Capability verification failed.
    CapabilityDenied {
        reason: CapError,
        object: ObjectKind,
        module: &'static str,
    },
    /// Requested object does not exist in the table.
    ObjectNotFound { object: ObjectKind },
    /// Effective rights are insufficient.
    InsufficientRights { had: Rights, needed: Rights },
}

impl core::fmt::Display for AccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CapabilityDenied {
                reason,
                object,
                module,
            } => write!(
                f,
                "access_control: [{module}] denied for {object}: {reason}"
            ),
            Self::ObjectNotFound { object } => {
                write!(f, "access_control: object not found: {object}")
            }
            Self::InsufficientRights { had: _, needed: _ } => {
                write!(f, "access_control: insufficient rights")
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// check_access — SINGLE ENTRY POINT (v6)
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies access to a protected object.
///
/// # Arguments
/// - `table`: capability table of calling process
/// - `token`: presented capability token
/// - `object`: requested object kind (for audit and error messages)
/// - `required`: required rights for this operation
/// - `caller`: calling module name ("ipc", "fs", "process", …)
///
/// # Behavior
/// - Calls `capability::verify::verify()` (INV-1)
/// - Logs result via `security::audit` (RULE SEC-AC-02)
/// - Returns `Ok(())` or `Err(AccessError)`
///
/// # Performance
/// Same complexity as `verify()`: O(1).
/// Audit overhead ≈ 1 atomic write into ring buffer.
#[inline]
fn current_audit_identity() -> (u32, u32, u16) {
    #[cfg(target_os = "none")]
    {
        let tcb = crate::scheduler::core::switch::current_thread_raw();
        if !tcb.is_null() {
            // SAFETY: current_thread_raw() returns the current running TCB on this CPU.
            let t = unsafe { &*tcb };
            let pid = t.pid.0;
            let uid = if pid == 0 {
                0
            } else {
                crate::process::core::registry::PROCESS_REGISTRY
                    .find_by_pid(crate::process::core::pid::Pid(pid))
                    .map(|pcb| pcb.euid_fast() as u16)
                    .unwrap_or(0)
            };
            return (pid, t.tid as u32, uid);
        }
    }
    (0, 0, 0)
}

#[inline]
pub fn check_access(
    table: &CapTable,
    token: CapToken,
    object: ObjectKind,
    required: Rights,
    caller: &'static str,
) -> Result<(), AccessError> {
    let (pid, tid, uid) = current_audit_identity();
    match cap_verify(table, token, required) {
        Ok(()) => {
            // ── Success — filterable audit log (Capability category)
            audit::log_event(
                AuditCategory::Capability,
                pid,
                tid,
                uid,
                0,
                0,
                AuditOutcome::Allow,
                [0u8; 8],
            );
            Ok(())
        }
        Err(CapError::ObjectNotFound) => {
            // Security violation — always logged (RULE AUDIT-02)
            audit::log_security_violation(pid, tid, uid, 0, [0u8; 8]);
            Err(AccessError::ObjectNotFound { object })
        }
        Err(CapError::InsufficientRights) => {
            audit::audit_capability_deny(pid, tid, uid, required.bits());
            Err(AccessError::InsufficientRights {
                had: Rights::NONE,
                needed: required,
            })
        }
        Err(e) => {
            audit::audit_capability_deny(pid, tid, uid, required.bits());
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

/// Initializes the access_control checker.
///
/// Called by `security::security_init()` at step 18 (v6 boot sequence).
/// Registers ObjectKind -> expected rights mappings in audit rules.
pub fn init() {
    // Register audit rules per object kind
    // (extensible: add an add_global_rule() per ObjectKind if needed)
    //
    // v6: no runtime dispatch table — rights are verified
    // directly by cap_verify(). This function exists for the boot
    // initialization hook (step 18) and for testability.
}
