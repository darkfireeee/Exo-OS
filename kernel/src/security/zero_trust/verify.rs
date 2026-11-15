// kernel/src/security/zero_trust/verify.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST VERIFY — Verification of every access (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE ZT-VERIFY-01: verify_access() is called for EVERY access to a
//   sensitive resource — syscall, IPC, device, crypto key.
//
// RULE ZT-VERIFY-02: A denial is ALWAYS logged into the TCB audit log.
//
// RULE ZT-VERIFY-03: In strict mode (SECURITY_STRICT), a DenyAndAlert
//   triggers an immediate alert to the security monitor (Ring 1).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{PrincipalId, SecurityContext};
use super::labels::SecurityLabel;
use super::policy::{global_policy, AccessRequest, PolicyAction, ResourceKind};
use crate::security::{audit, shield_feed};

/// Bitmask of trusted Ring1 PIDs. The compact form covers early PIDs
/// of canonical servers; PIDs >= 64 take the slow path.
static RING1_TRUSTED_MASK: AtomicU64 = AtomicU64::new((1u64 << 1) | (1u64 << 2));

#[inline(always)]
fn ring1_bit(pid: u32) -> Option<u64> {
    if pid < 64 {
        Some(1u64 << pid)
    } else {
        None
    }
}

pub fn register_ring1_pid(pid: u32) {
    if let Some(bit) = ring1_bit(pid) {
        RING1_TRUSTED_MASK.fetch_or(bit, Ordering::Release);
    }
}

pub fn unregister_ring1_pid(pid: u32) {
    if let Some(bit) = ring1_bit(pid) {
        RING1_TRUSTED_MASK.fetch_and(!bit, Ordering::Release);
    }
}

pub fn ring1_trusted_mask() -> u64 {
    RING1_TRUSTED_MASK.load(Ordering::Acquire)
}

#[inline]
fn is_ring1_pid_trusted(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if let Some(bit) = ring1_bit(pid) {
        if (RING1_TRUSTED_MASK.load(Ordering::Acquire) & bit) != 0 {
            return true;
        }
    }
    pid >= 64
        && crate::security::ipc_policy::can_inject_src_pid(crate::process::core::pid::Pid(pid))
}

#[inline]
pub fn ring1_pair_trusted(sender_pid: u32, receiver_pid: u32) -> bool {
    is_ring1_pid_trusted(sender_pid) && is_ring1_pid_trusted(receiver_pid)
}

// ─────────────────────────────────────────────────────────────────────────────
// AccessError — access verification error
// ─────────────────────────────────────────────────────────────────────────────

/// Error returned by `verify_access()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// Access denied (policy).
    Denied,
    /// Access denied with security alert raised.
    DeniedAlert,
    /// Invalid security context.
    InvalidContext,
}

impl core::fmt::Display for AccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Denied => write!(f, "zero-trust: access denied"),
            Self::DeniedAlert => write!(f, "zero-trust: access denied (security alert raised)"),
            Self::InvalidContext => write!(f, "zero-trust: invalid security context"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// verify_access — main entry point
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn kind_to_feed_type(kind: ResourceKind) -> u8 {
    match kind {
        ResourceKind::Syscall => shield_feed::event_type::SYSCALL,
        ResourceKind::IpcEndpoint => shield_feed::event_type::IPC,
        ResourceKind::DmaChannel | ResourceKind::MemoryRegion => shield_feed::event_type::MEMORY,
        _ => shield_feed::event_type::CAPABILITY,
    }
}

/// Verifies an access to a resource according to the Zero-Trust policy.
///
/// # Parameters
/// * `subject` — security context of the requesting thread
/// * `resource_kind` — type of accessed resource
/// * `object_label` — security label of the resource (confidentiality + integrity)
/// * `is_write` — true if write, false if read
/// * `context_data` — additional data for audit (ObjectId, syscall no…)
///
/// # Returns
/// * `Ok(())` if access is granted
/// * `Err(AccessError::Denied)` if denied
/// * `Err(AccessError::DeniedAlert)` if denied with alert
#[inline]
pub fn verify_access(
    subject: &SecurityContext,
    resource_kind: ResourceKind,
    object_label: SecurityLabel,
    is_write: bool,
    context_data: u64,
) -> Result<(), AccessError> {
    let req = AccessRequest {
        subject,
        resource_kind,
        object_label,
        is_write,
        context_data,
    };

    let decision = global_policy().evaluate(&req);

    match decision {
        PolicyAction::Allow => Ok(()),
        PolicyAction::Deny => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            shield_feed::push_event(
                subject.principal.pid,
                kind_to_feed_type(resource_kind),
                shield_feed::severity::MEDIUM,
                resource_kind as u32,
                context_data,
                is_write as u64,
            );
            Err(AccessError::Denied)
        }
        PolicyAction::DenyAndAudit => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            shield_feed::push_event(
                subject.principal.pid,
                kind_to_feed_type(resource_kind),
                shield_feed::severity::HIGH,
                resource_kind as u32,
                context_data,
                is_write as u64,
            );
            Err(AccessError::Denied)
        }
        PolicyAction::DenyAndAlert => {
            audit::log_security_violation(
                subject.principal.pid,
                subject.principal.tid,
                0,
                1,
                context_data.to_le_bytes(),
            );
            shield_feed::push_event(
                subject.principal.pid,
                kind_to_feed_type(resource_kind),
                shield_feed::severity::CRITICAL,
                resource_kind as u32,
                context_data,
                is_write as u64,
            );
            Err(AccessError::DeniedAlert)
        }
    }
}

/// Shortcut: verifies a file read access.
#[inline(always)]
pub fn verify_file_read(
    subject: &SecurityContext,
    object_label: SecurityLabel,
    inode_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::FileInode,
        object_label,
        false,
        inode_id,
    )
}

/// Shortcut: verifies a file write access.
#[inline(always)]
pub fn verify_file_write(
    subject: &SecurityContext,
    object_label: SecurityLabel,
    inode_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::FileInode,
        object_label,
        true,
        inode_id,
    )
}

/// Shortcut: verifies an IPC access.
#[inline(always)]
pub fn verify_ipc_access(
    subject: &SecurityContext,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    let receiver_pid = (endpoint_id >> 32) as u32;
    if receiver_pid != 0 && ring1_pair_trusted(subject.principal.pid, receiver_pid) {
        return Ok(());
    }
    verify_ipc_peer_access(subject, None, ep_label, endpoint_id, is_send)
}

/// Verifies an IPC access when receiver PID is known to the caller.
#[inline(always)]
pub fn verify_ipc_access_between(
    subject: &SecurityContext,
    receiver: PrincipalId,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    verify_ipc_peer_access(subject, Some(receiver.pid), ep_label, endpoint_id, is_send)
}

#[inline]
fn verify_ipc_peer_access(
    subject: &SecurityContext,
    receiver_pid: Option<u32>,
    ep_label: SecurityLabel,
    endpoint_id: u64,
    is_send: bool,
) -> Result<(), AccessError> {
    if receiver_pid
        .map(|pid| ring1_pair_trusted(subject.principal.pid, pid))
        .unwrap_or(false)
    {
        return Ok(());
    }

    verify_access(
        subject,
        ResourceKind::IpcEndpoint,
        ep_label,
        is_send,
        endpoint_id,
    )
}

/// Shortcut: verifies a cryptographic key access.
#[inline(always)]
pub fn verify_crypto_key_access(
    subject: &SecurityContext,
    key_label: SecurityLabel,
    key_id: u64,
) -> Result<(), AccessError> {
    verify_access(subject, ResourceKind::CryptoKey, key_label, false, key_id)
}

/// Shortcut: verifies a DMA access.
#[inline(always)]
pub fn verify_dma_access(
    subject: &SecurityContext,
    chan_label: SecurityLabel,
    channel_id: u64,
) -> Result<(), AccessError> {
    verify_access(
        subject,
        ResourceKind::DmaChannel,
        chan_label,
        true,
        channel_id,
    )
}

/// Verifies a syscall invocation in a sandboxed context.
#[inline]
pub fn verify_syscall(subject: &SecurityContext, syscall_no: u64) -> Result<(), AccessError> {
    // Syscalls have the kernel label — sandbox restriction verification
    let kernel_label = SecurityLabel::kernel();
    verify_access(
        subject,
        ResourceKind::Syscall,
        kernel_label,
        false,
        syscall_no,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(pid: u32) -> PrincipalId {
        PrincipalId {
            uid: 1000,
            gid: 1000,
            pid,
            tid: pid,
            ns_id: 0,
        }
    }

    #[test]
    fn ring1_pair_bypasses_full_mls_check() {
        register_ring1_pid(40);
        register_ring1_pid(41);
        let subject = SecurityContext::new_normal(principal(40));
        let endpoint_id = (41u64 << 32) | 7;

        assert_eq!(
            verify_ipc_access(&subject, SecurityLabel::kernel(), endpoint_id, true),
            Ok(())
        );

        unregister_ring1_pid(40);
        unregister_ring1_pid(41);
    }

    #[test]
    fn unregister_removes_ring1_fast_path() {
        register_ring1_pid(42);
        register_ring1_pid(43);
        assert!(ring1_pair_trusted(42, 43));

        unregister_ring1_pid(42);
        assert!(!ring1_pair_trusted(42, 43));

        unregister_ring1_pid(43);
    }

    /// TIER 1.1 — `verify_syscall` TRULY enforces context restrictions:
    /// a process holding `NO_FORK` has `fork` denied, but retains benign
    /// syscalls; an unrestricted process succeeds (boot-safe).
    #[test]
    fn syscall_restriction_is_enforced() {
        use super::super::context::{restriction_flags, TrustLevel};
        use crate::syscall::numbers::{SYS_FORK, SYS_READ};

        // Process restricted NO_FORK.
        let restricted = SecurityContext::for_process(
            principal(60),
            TrustLevel::Normal,
            restriction_flags::NO_FORK,
        );
        assert_eq!(
            verify_syscall(&restricted, SYS_FORK),
            Err(AccessError::Denied),
            "fork doit être refusé sous NO_FORK"
        );
        assert!(
            verify_syscall(&restricted, SYS_READ).is_ok(),
            "un syscall bénin reste autorisé"
        );

        // Unrestricted process: fork authorized.
        let free = SecurityContext::for_process(principal(61), TrustLevel::Normal, 0);
        assert!(
            verify_syscall(&free, SYS_FORK).is_ok(),
            "sans restriction, fork passe (boot-safe)"
        );
    }
}
