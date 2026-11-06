// kernel/src/security/zero_trust/process_state.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST PROCESS STATE — Persistent per-process security state (TIER 1.1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// The `policy.rs` engine is REAL (Bell-LaPadula/Biba MLS, restrictions, per-
// resource rules) but remained INERT for syscalls: dispatch reconstructed a
// fresh `SecurityContext::new_normal` (restrictions=0, fixed trust) on each call,
// ignoring the actual state of the process → de facto "TRUST_ALL" behavior.
//
// This module holds this actual state and constructs the context consulted at each
// syscall (`dispatch.rs`):
//   • sandbox/pledge restrictions persisted per PID (harden-only);
//   • trust level derived from system state (init/Ring 1/normal).
//
// STORAGE: lock-free array indexed by PID (`[AtomicU64; MAX_TRACKED_PIDS]`).
//   - Default (slot=0) = NO restriction → identical to previous behavior
//     (BOOT-SAFE: nothing is restricted until a process explicitly opts in
//     via `restrict_process`).
//   - Monotonic: `restrict_process` only ADDS restriction bits
//     (RULE ZT-02 / PLEDGE-01: rights can only decrease).
//   - Out-of-bounds PID (>= MAX): untrackable → `restrict_process` fails
//     (fail-closed at opt-in site); reading returns 0 (consistent — such PID
//     could never have been restricted).
//   - Recycling: the slot is reset to 0 upon PID release (cf. lifecycle/wait).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{restriction_flags, PrincipalId, SecurityContext, TrustLevel};
use super::verify::ring1_trusted_mask;
use crate::syscall::numbers;

/// Number of trackable PIDs without lock. Covers the entirety of Exo-OS PID space
/// (PID_MAX = 32,767, thus 32,768 slots to cover PIDs 0..=32767 without fail-open).
pub const MAX_TRACKED_PIDS: usize = 32768;

/// PID of the init process — cannot be restricted (RULE PLEDGE-03).
const INIT_PID: u32 = 1;

/// Restriction state per PID. `0` = no restriction.
static PROCESS_RESTRICTIONS: [AtomicU64; MAX_TRACKED_PIDS] =
    [const { AtomicU64::new(0) }; MAX_TRACKED_PIDS];

/// Adds restrictions to process `pid` (monotonic — can only HARDEN).
///
/// Rejects init (PID 1, RULE PLEDGE-03), PID 0, and out-of-range PIDs.
/// Returns `true` if the restriction was applied.
pub fn restrict_process(pid: u32, flags: u64) -> bool {
    if pid == 0 || pid == INIT_PID {
        return false;
    }
    let idx = pid as usize;
    if idx >= MAX_TRACKED_PIDS {
        return false;
    }
    PROCESS_RESTRICTIONS[idx].fetch_or(flags, Ordering::Release);
    true
}

/// Active restrictions of process `pid` (0 = none).
#[inline]
pub fn process_restrictions(pid: u32) -> u64 {
    let idx = pid as usize;
    if idx >= MAX_TRACKED_PIDS {
        return 0;
    }
    PROCESS_RESTRICTIONS[idx].load(Ordering::Acquire)
}

/// Resets a PID's state. Called upon PID release (recycling) so
/// a future process reusing this number starts WITHOUT inherited restrictions.
pub fn clear_process_restrictions(pid: u32) {
    let idx = pid as usize;
    if idx < MAX_TRACKED_PIDS {
        PROCESS_RESTRICTIONS[idx].store(0, Ordering::Release);
    }
}

/// Fork inheritance: child receives AT LEAST the parent's restrictions
/// (RULE ZT-03 / SAND-03: a child cannot be less restricted than its parent).
pub fn inherit_restrictions(parent_pid: u32, child_pid: u32) {
    let parent = process_restrictions(parent_pid);
    if parent != 0 {
        let _ = restrict_process(child_pid, parent);
    }
}

/// **Authenticated** trust level of a PID, derived from actual system state.
///
/// - init (PID 1) → System (most privileged process);
/// - trusted Ring 1 service (mask maintained by `ipc_policy`) → Trusted;
/// - everything else (apps, shell) → Normal.
pub fn trust_for_pid(pid: u32) -> TrustLevel {
    if pid == INIT_PID {
        return TrustLevel::System;
    }
    if pid < 64 && (ring1_trusted_mask() & (1u64 << pid)) != 0 {
        return TrustLevel::Trusted;
    }
    TrustLevel::Normal
}

/// Constructs the ACTUAL `SecurityContext` of the calling process: trust level
/// derived from system state + persisted restrictions. Replaces the inert
/// `new_normal` previously hardcoded in dispatch (TIER 1.1).
pub fn context_for_caller(pid: u32, tid: u32) -> SecurityContext {
    let principal = PrincipalId {
        uid: 0,
        gid: 0,
        pid,
        tid,
        ns_id: 0,
    };
    SecurityContext::for_process(principal, trust_for_pid(pid), process_restrictions(pid))
}

/// Mask of restrictions that FORBID syscall `nr`. If caller holds
/// one of these restrictions, the syscall is rejected by `policy::check_restrictions`.
///
/// Mapping (Linux-compat numbers, cf. [`crate::syscall::numbers`]):
///   - process creation (fork/clone/vfork) → `NO_FORK | NO_PROCESS_CREATE`
///   - exec (execve)                       → `NO_EXEC | NO_PROCESS_CREATE`
///   - network (socket/connect/bind/listen/ → `NO_NETWORK`
///     accept/sendto/recvfrom/…)
#[inline]
pub fn syscall_restriction_mask(nr: u64) -> u64 {
    use restriction_flags::*;
    match nr {
        numbers::SYS_FORK | numbers::SYS_CLONE | numbers::SYS_VFORK => NO_FORK | NO_PROCESS_CREATE,
        numbers::SYS_EXECVE => NO_EXEC | NO_PROCESS_CREATE,
        numbers::SYS_SOCKET
        | numbers::SYS_CONNECT
        | numbers::SYS_BIND
        | numbers::SYS_LISTEN
        | numbers::SYS_ACCEPT
        | numbers::SYS_SENDTO
        | numbers::SYS_RECVFROM
        | numbers::SYS_SENDMSG
        | numbers::SYS_RECVMSG
        | numbers::SYS_SOCKETPAIR => NO_NETWORK,
        _ => 0,
    }
}

/// Translates a bitmask of **pledge promises** (OpenBSD-like, `pledge_flags`) into
/// **zero-trust restrictions**: any capacity NOT promised becomes an enforced
/// restriction at the syscall boundary (cf. `syscall_restriction_mask`). TIER 2.10 —
/// bridge between userspace `pledge()` API and TIER 1.1 enforcement.
///
/// Mapping (missing promise → restriction):
///   - `PROC` (process creation) absent → `NO_FORK | NO_PROCESS_CREATE`
///   - `EXEC` absent                    → `NO_EXEC`
///   - `INET` (network) absent           → `NO_NETWORK`
///
/// Only actually enforced capabilities are mapped (fork/exec/network);
/// other pledges (rpath/wpath/…) will be added when per-syscall FS/IO
/// enforcement exists. Conservative: never restricts more than known flags.
pub fn pledge_promises_to_restrictions(promises: u64) -> u64 {
    use crate::security::isolation::pledge::pledge_flags;
    use restriction_flags::*;
    let mut r = 0u64;
    if promises & pledge_flags::PROC == 0 {
        r |= NO_FORK | NO_PROCESS_CREATE;
    }
    if promises & pledge_flags::EXEC == 0 {
        r |= NO_EXEC;
    }
    if promises & pledge_flags::INET == 0 {
        r |= NO_NETWORK;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test PIDs chosen high to avoid collisions with any potential
    // system PIDs (and remaining < MAX_TRACKED_PIDS).
    const P_A: u32 = 700;
    const P_B: u32 = 701;
    const P_CHILD: u32 = 702;

    fn reset(pids: &[u32]) {
        for &p in pids {
            clear_process_restrictions(p);
        }
    }

    /// Default = no restriction (boot-safe). `restrict_process` hardens; init
    /// and out-of-range PIDs are rejected.
    #[test]
    fn restrict_is_monotonic_and_guarded() {
        reset(&[P_A]);
        assert_eq!(process_restrictions(P_A), 0, "défaut = non restreint");

        assert!(restrict_process(P_A, restriction_flags::NO_FORK));
        assert!(process_restrictions(P_A) & restriction_flags::NO_FORK != 0);

        // Monotonic addition: the first restriction remains present.
        assert!(restrict_process(P_A, restriction_flags::NO_NETWORK));
        let r = process_restrictions(P_A);
        assert!(r & restriction_flags::NO_FORK != 0 && r & restriction_flags::NO_NETWORK != 0);

        // init cannot be restricted (RULE PLEDGE-03).
        assert!(!restrict_process(INIT_PID, restriction_flags::NO_FORK));
        assert_eq!(process_restrictions(INIT_PID), 0);

        // Out-of-range PID → untrackable.
        assert!(!restrict_process(
            MAX_TRACKED_PIDS as u32,
            restriction_flags::NO_FORK
        ));
        assert_eq!(process_restrictions(MAX_TRACKED_PIDS as u32 + 5), 0);

        reset(&[P_A]);
    }

    /// Recycling (clear) starts from a blank slate.
    #[test]
    fn clear_resets_slot() {
        reset(&[P_B]);
        assert!(restrict_process(P_B, restriction_flags::SANDBOX_FULL));
        assert!(process_restrictions(P_B) != 0);
        clear_process_restrictions(P_B);
        assert_eq!(process_restrictions(P_B), 0);
    }

    /// Fork child inherits (at least) parent restrictions.
    #[test]
    fn child_inherits_parent_restrictions() {
        reset(&[P_A, P_CHILD]);
        assert!(restrict_process(P_A, restriction_flags::NO_EXEC));
        inherit_restrictions(P_A, P_CHILD);
        assert!(
            process_restrictions(P_CHILD) & restriction_flags::NO_EXEC != 0,
            "le fils doit hériter NO_EXEC"
        );
        reset(&[P_A, P_CHILD]);
    }

    /// Syscall->restriction mapping covers create-process / exec / network.
    #[test]
    fn syscall_mask_maps_dangerous_syscalls() {
        use restriction_flags::*;
        assert!(syscall_restriction_mask(numbers::SYS_FORK) & NO_FORK != 0);
        assert!(syscall_restriction_mask(numbers::SYS_CLONE) & NO_PROCESS_CREATE != 0);
        assert!(syscall_restriction_mask(numbers::SYS_EXECVE) & NO_EXEC != 0);
        assert!(syscall_restriction_mask(numbers::SYS_SOCKET) & NO_NETWORK != 0);
        // Benign syscall (read=0) is never filtered by this mechanism.
        assert_eq!(syscall_restriction_mask(numbers::SYS_READ), 0);
    }

    /// `trust_for_pid`: init = System, unknown = Normal.
    #[test]
    fn trust_derivation_init_and_default() {
        assert_eq!(trust_for_pid(INIT_PID), TrustLevel::System);
        assert_eq!(trust_for_pid(P_A), TrustLevel::Normal);
    }

    /// TIER 2.10 — `pledge()`: missing promise becomes an enforced restriction;
    /// all promises → nothing to restrict.
    #[test]
    fn pledge_maps_absent_promises_to_restrictions() {
        use crate::security::isolation::pledge::pledge_flags;
        use restriction_flags::*;

        // STDIO alone (not PROC/EXEC/INET) → fork+exec+network restricted.
        let r = pledge_promises_to_restrictions(pledge_flags::STDIO);
        assert!(r & NO_FORK != 0 && r & NO_PROCESS_CREATE != 0);
        assert!(r & NO_EXEC != 0);
        assert!(r & NO_NETWORK != 0);

        // PROC|EXEC|INET promised → no restriction (on enforced flags).
        let none = pledge_promises_to_restrictions(
            pledge_flags::PROC | pledge_flags::EXEC | pledge_flags::INET,
        );
        assert_eq!(none, 0);

        // INET alone → network allowed, fork/exec restricted.
        let net_only = pledge_promises_to_restrictions(pledge_flags::INET);
        assert_eq!(net_only & NO_NETWORK, 0);
        assert!(net_only & NO_FORK != 0 && net_only & NO_EXEC != 0);
    }
}
