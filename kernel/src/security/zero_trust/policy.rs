// kernel/src/security/zero_trust/policy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST POLICY — Security policy engine
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module implements the Exo-OS Zero-Trust policy engine.
// EVERY access to a resource must be evaluated by this engine.
//
// Zero-Trust PRINCIPLE: "Never trust, always verify"
//   • No implicit trust based on location (internal network, Ring 1…)
//   • Authentication + authorization for EVERY access
//   • Access granted on the minimum necessary (least privilege)
//   • Continuous access monitoring (logging)
//
// RULE ZT-POLICY-01: deny_by_default() is the default action.
//   If no rule matches → access denied.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{SecurityContext, TrustLevel};
use super::labels::SecurityLabel;

// ─────────────────────────────────────────────────────────────────────────────
// PolicyAction — policy decision
// ─────────────────────────────────────────────────────────────────────────────

/// Decision made by the policy engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Access granted.
    Allow,
    /// Access denied.
    Deny,
    /// Access denied + mandatory audit.
    DenyAndAudit,
    /// Access denied + security alert.
    DenyAndAlert,
}

impl PolicyAction {
    #[inline(always)]
    pub fn is_allow(self) -> bool {
        self == Self::Allow
    }

    #[inline(always)]
    pub fn is_deny(self) -> bool {
        !self.is_allow()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResourceKind — type of accessed resource
// ─────────────────────────────────────────────────────────────────────────────

/// Type of resource subject to policy verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceKind {
    /// IPC endpoint.
    IpcEndpoint = 0,
    /// Memory region.
    MemoryRegion = 1,
    /// File inode.
    FileInode = 2,
    /// Physical device.
    Device = 3,
    /// Thread belonging to another process.
    Thread = 4,
    /// Namespace.
    Namespace = 5,
    /// Cryptographic key.
    CryptoKey = 6,
    /// DMA channel.
    DmaChannel = 7,
    /// System syscall (for sandboxing).
    Syscall = 8,
}

// ─────────────────────────────────────────────────────────────────────────────
// AccessRequest — access request to evaluate
// ─────────────────────────────────────────────────────────────────────────────

/// Access request passed to the policy engine.
pub struct AccessRequest<'a> {
    /// Requestor context.
    pub subject: &'a SecurityContext,
    /// Type of resource accessed.
    pub resource_kind: ResourceKind,
    /// Security label of the resource.
    pub object_label: SecurityLabel,
    /// Requested operation: true = write, false = read.
    pub is_write: bool,
    /// Supplementary data (syscall no, address, etc.).
    pub context_data: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroTrustPolicy — main engine
// ─────────────────────────────────────────────────────────────────────────────

/// Zero-Trust policy engine.
///
/// # Evaluation algorithm
/// 1. Insufficient trust level → DenyAndAudit
/// 2. Active restriction for this resource kind → DenyAndAudit
/// 3. MLS verification (Bell-LaPadula + Biba) → Deny on violation
/// 4. Resource kind-specific rule
/// 5. Allow (default allow for Kernel/System, Deny for remainder if unmatched)
pub struct ZeroTrustPolicy {
    evaluations: AtomicU64,
    denials: AtomicU64,
    alerts: AtomicU64,
}

impl ZeroTrustPolicy {
    pub const fn new() -> Self {
        Self {
            evaluations: AtomicU64::new(0),
            denials: AtomicU64::new(0),
            alerts: AtomicU64::new(0),
        }
    }

    /// Evaluates an access request — core of the Zero-Trust engine.
    pub fn evaluate(&self, req: &AccessRequest<'_>) -> PolicyAction {
        self.evaluations.fetch_add(1, Ordering::Relaxed);

        let trust = req.subject.trust_level();

        // ── 1. Untrusted threads: deny all except Public read ────────────────
        if trust == TrustLevel::Untrusted {
            // Untrusted can only read Public resources
            if req.is_write
                || req.object_label.confidentiality != super::labels::ConfidentialityLevel::Public
            {
                self.denials.fetch_add(1, Ordering::Relaxed);
                req.subject.record_deny();
                return PolicyAction::DenyAndAudit;
            }
        }

        // ── 2. Active restrictions ───────────────────────────────────────────
        let action = self.check_restrictions(req, trust);
        if action.is_deny() {
            self.denials.fetch_add(1, Ordering::Relaxed);
            req.subject.record_deny();
            return action;
        }

        // ── 3. MLS verification ───────────────────────────────────────────────
        // FIX-ZT-SYSCALL: INVOCATION of a syscall is NOT a Bell-LaPadula data
        // flow. Syscall authorization is delegated to `check_restrictions` /
        // sandbox (capabilities, seccomp) — cf. `ResourceKind::Syscall` branch
        // of `check_restrictions` which returns Allow.
        // Applying no-read-up against `kernel()` label (TopSecret/Critical)
        // here would deny *all* syscalls from a `user_default()` thread
        // (Internal does not dominate TopSecret), rendering userspace
        // completely inoperable (PID 1 can neither write, fork, nor do IPC).
        // MLS thus applies only to DATA resource accesses (files, IPC,
        // memory, devices), not to the act of calling a syscall.
        let mls_applies = !matches!(req.resource_kind, ResourceKind::Syscall);
        if mls_applies {
            let subject_label = req.subject.label();
            let mls_ok = if req.is_write {
                subject_label.can_write(req.object_label)
            } else {
                subject_label.can_read(req.object_label)
            };

            if !mls_ok {
                self.denials.fetch_add(1, Ordering::Relaxed);
                req.subject.record_deny();
                // MLS violation: potentially an exploit → alert
                if req.object_label.confidentiality >= super::labels::ConfidentialityLevel::Secret {
                    self.alerts.fetch_add(1, Ordering::Relaxed);
                    return PolicyAction::DenyAndAlert;
                }
                return PolicyAction::DenyAndAudit;
            }
        }

        // ── 4. Resource-specific rules ────────────────────────────────────────
        let resource_action = self.check_resource_rules(req, trust);
        if resource_action.is_deny() {
            self.denials.fetch_add(1, Ordering::Relaxed);
            req.subject.record_deny();
            return resource_action;
        }

        // ── 5. Allow ──────────────────────────────────────────────────────────
        req.subject.record_allow();
        PolicyAction::Allow
    }

    fn check_restrictions(&self, req: &AccessRequest<'_>, _trust: TrustLevel) -> PolicyAction {
        use super::context::restriction_flags::*;

        match req.resource_kind {
            ResourceKind::IpcEndpoint => {
                // No active restriction forbidding IPC
                PolicyAction::Allow
            }
            ResourceKind::FileInode => {
                if req.subject.has_restriction(NO_FS) {
                    return PolicyAction::DenyAndAudit;
                }
                if req.is_write && req.subject.has_restriction(FS_READONLY) {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            ResourceKind::Syscall => {
                // TIER 1.1: enforce ACTUAL sandbox/pledge restrictions of the
                // calling process. `context_data` carries the syscall number;
                // if caller holds a restriction forbidding this syscall
                // (NO_FORK/NO_EXEC/NO_NETWORK/NO_PROCESS_CREATE), deny + audit.
                // Default (unrestricted process) → mask 0 → Allow (boot-safe).
                let forbidding = super::process_state::syscall_restriction_mask(req.context_data);
                if forbidding != 0 && req.subject.has_restriction(forbidding) {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            ResourceKind::Device => {
                // Devices: restricted to Trusted+ threads
                PolicyAction::Allow
            }
            _ => PolicyAction::Allow,
        }
    }

    fn check_resource_rules(&self, req: &AccessRequest<'_>, trust: TrustLevel) -> PolicyAction {
        match req.resource_kind {
            ResourceKind::CryptoKey => {
                // Crypto keys: minimum Trusted
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            ResourceKind::DmaChannel => {
                // DMA: reserved for drivers (Trusted+)
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAlert;
                }
                PolicyAction::Allow
            }
            ResourceKind::Device => {
                if trust < TrustLevel::Trusted {
                    return PolicyAction::DenyAndAudit;
                }
                PolicyAction::Allow
            }
            _ => PolicyAction::Allow,
        }
    }

    /// Engine statistics.
    pub fn stats(&self) -> PolicyStats {
        PolicyStats {
            evaluations: self.evaluations.load(Ordering::Relaxed),
            denials: self.denials.load(Ordering::Relaxed),
            alerts: self.alerts.load(Ordering::Relaxed),
        }
    }
}

impl Default for ZeroTrustPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Policy engine statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct PolicyStats {
    pub evaluations: u64,
    pub denials: u64,
    pub alerts: u64,
}

// Global singleton
static GLOBAL_POLICY: ZeroTrustPolicy = ZeroTrustPolicy::new();

pub fn global_policy() -> &'static ZeroTrustPolicy {
    &GLOBAL_POLICY
}
