// kernel/src/security/zero_trust/context.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SECURITY CONTEXT — Per thread/process security context (Zero-Trust)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Each thread possesses a SecurityContext describing:
//   • Its trust level (TrustLevel)
//   • MLS-like security labels (confidentiality, integrity)
//   • Principal identity (UID, GID, namespace)
//   • Active restrictions (sandbox, pledge)
//
// RULE ZT-01: Every verified access MUST go through this context.
// RULE ZT-02: TrustLevel can only DECREASE — never increase without re-authentication.
// RULE ZT-03: Child contexts inherit a subset of the parent context.
// ═══════════════════════════════════════════════════════════════════════════════

use super::labels::SecurityLabel;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// TrustLevel — trust level
// ─────────────────────────────────────────────────────────────────────────────

/// Trust level of a thread.
/// Order: System > Kernel > Trusted > Normal > Restricted > Untrusted
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TrustLevel {
    /// Completely untrusted thread (strict sandbox).
    Untrusted = 0,
    /// Restricted thread (active pledge, limited syscalls).
    Restricted = 1,
    /// Normal thread (standard user application).
    Normal = 2,
    /// Trusted thread (verified system service).
    Trusted = 3,
    /// Kernel thread.
    Kernel = 4,
    /// System thread (init, critical driver).
    System = 5,
}

impl TrustLevel {
    /// Returns the maximum level granted to a child thread (inheritance).
    pub fn inherit(self) -> Self {
        // A child cannot be more trusted than its parent
        match self {
            Self::System => Self::Trusted, // child of System = max Trusted
            Self::Kernel => Self::Trusted,
            Self::Trusted => Self::Normal,
            Self::Normal => Self::Normal,
            Self::Restricted => Self::Restricted,
            Self::Untrusted => Self::Untrusted,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PrincipalId — principal identity
// ─────────────────────────────────────────────────────────────────────────────

/// Identity of a principal (thread or process).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalId {
    /// POSIX UID.
    pub uid: u32,
    /// POSIX GID.
    pub gid: u32,
    /// Process PID.
    pub pid: u32,
    /// Thread TID.
    pub tid: u32,
    /// PID namespace (namespace isolation).
    pub ns_id: u32,
}

impl PrincipalId {
    pub const ROOT: Self = Self {
        uid: 0,
        gid: 0,
        pid: 0,
        tid: 0,
        ns_id: 0,
    };

    pub fn is_root(&self) -> bool {
        self.uid == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SecurityContext — thread security context
// ─────────────────────────────────────────────────────────────────────────────

/// Zero-Trust security context of a thread.
///
/// # Invariants (RULE ZT-02)
/// - `trust_level` can only decrease via `downgrade()`.
/// - `label` cannot be elevated without re-authentication.
#[repr(C)]
pub struct SecurityContext {
    /// Principal identity.
    pub principal: PrincipalId,
    /// Current trust level.
    trust_level: AtomicU32,
    /// MLS security label.
    label: SecurityLabel,
    /// Active restriction flags.
    restrictions: AtomicU64,
    /// Number of denied accesses since creation.
    denied_count: AtomicU64,
    /// Number of granted accesses.
    allowed_count: AtomicU64,
}

/// Restriction flags.
pub mod restriction_flags {
    /// Restrict: limited syscalls (active pledge).
    pub const PLEDGE_ACTIVE: u64 = 1 << 0;
    /// Restrict: no network.
    pub const NO_NETWORK: u64 = 1 << 1;
    /// Restrict: no file access outside tmpfs.
    pub const NO_FS: u64 = 1 << 2;
    /// Restrict: no fork.
    pub const NO_FORK: u64 = 1 << 3;
    /// Restrict: no exec.
    pub const NO_EXEC: u64 = 1 << 4;
    /// Restrict: full sandbox (seccomp-like).
    pub const SANDBOX_FULL: u64 = 1 << 5;
    /// Restrict: read-only FS.
    pub const FS_READONLY: u64 = 1 << 6;
    /// Restrict: no process creation.
    pub const NO_PROCESS_CREATE: u64 = 1 << 7;
    /// Restrict: no abstract AF_UNIX namespace (`sun_path[0] == 0`) — S3.
    pub const NO_UNIX_ABSTRACT: u64 = 1 << 8;
    /// Restrict: no runtime `dlopen()` in `ld-exo.so` — S2.
    pub const NO_DLOPEN: u64 = 1 << 9;
    /// Restrict: no direct IPC to critical Ring 1 services (`exo compat run`) — S2.
    pub const NO_RING1_CRITICAL_IPC: u64 = 1 << 10;
    /// Restrict: no AF_INET / AF_INET6 sockets when `INET` promise is absent — S3.
    pub const NO_INET: u64 = 1 << 11;
}

// SAFETY: SecurityContext contains only atomic primitives.
unsafe impl Send for SecurityContext {}
unsafe impl Sync for SecurityContext {}

impl SecurityContext {
    /// Creates a context for a normal thread.
    pub fn new_normal(principal: PrincipalId) -> Self {
        Self {
            principal,
            trust_level: AtomicU32::new(TrustLevel::Normal as u32),
            label: SecurityLabel::user_default(),
            restrictions: AtomicU64::new(0),
            denied_count: AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Creates a context for a kernel thread.
    pub fn new_kernel(principal: PrincipalId) -> Self {
        Self {
            principal,
            trust_level: AtomicU32::new(TrustLevel::Kernel as u32),
            label: SecurityLabel::kernel(),
            restrictions: AtomicU64::new(0),
            denied_count: AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Constructs a context for a process from its **authenticated** trust
    /// level and its **persisted restrictions** (per-process zero-trust state —
    /// cf. [`super::process_state`]).
    ///
    /// TIER 1.1: replaces inert `new_normal` (restrictions=0, fixed trust)
    /// previously hardcoded in dispatch. The level is set at creation
    /// (RULE ZT-02: it can only decrease thereafter). The label derives from
    /// the level (Trusted+ = kernel-label; below = user_default) — without
    /// impact on the syscall path (MLS does not apply to the act of calling
    /// a syscall, cf. `policy::evaluate`).
    pub fn for_process(principal: PrincipalId, trust: TrustLevel, restrictions: u64) -> Self {
        let label = if (trust as u8) >= (TrustLevel::Trusted as u8) {
            SecurityLabel::kernel()
        } else {
            SecurityLabel::user_default()
        };
        Self {
            principal,
            trust_level: AtomicU32::new(trust as u32),
            label,
            restrictions: AtomicU64::new(restrictions),
            denied_count: AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Returns the current trust level.
    #[inline(always)]
    pub fn trust_level(&self) -> TrustLevel {
        match self.trust_level.load(Ordering::Acquire) {
            0 => TrustLevel::Untrusted,
            1 => TrustLevel::Restricted,
            2 => TrustLevel::Normal,
            3 => TrustLevel::Trusted,
            4 => TrustLevel::Kernel,
            5 => TrustLevel::System,
            _ => TrustLevel::Untrusted,
        }
    }

    /// Downgrades the trust level — RULE ZT-02.
    /// Can NOT increase the level (race-free atomic CAS).
    pub fn downgrade(&self, new_level: TrustLevel) {
        let target = new_level as u32;
        let _ = self.trust_level.fetch_update(
            Ordering::Release,
            Ordering::Acquire,
            |curr| {
                if target < curr {
                    Some(target)
                } else {
                    None
                }
            },
        );
    }

    /// Returns the security label.
    #[inline(always)]
    pub fn label(&self) -> SecurityLabel {
        self.label
    }

    /// Enables a restriction.
    pub fn add_restriction(&self, flag: u64) {
        self.restrictions.fetch_or(flag, Ordering::Release);
    }

    /// Removes a restriction (reserved for authorized escalation operations).
    pub fn remove_restriction(&self, flag: u64) {
        self.restrictions.fetch_and(!flag, Ordering::Release);
    }

    /// Checks if a restriction is active.
    #[inline(always)]
    pub fn has_restriction(&self, flag: u64) -> bool {
        (self.restrictions.load(Ordering::Acquire) & flag) != 0
    }

    /// Increments the denial counter — for ZT monitoring.
    #[inline(always)]
    pub(super) fn record_deny(&self) {
        self.denied_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the granted access counter.
    #[inline(always)]
    pub(super) fn record_allow(&self) {
        self.allowed_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Creates a child context with inherited level (RULE ZT-03).
    pub fn derive_child(&self, child_principal: PrincipalId) -> SecurityContext {
        let child_trust = self.trust_level().inherit();
        SecurityContext {
            principal: child_principal,
            trust_level: AtomicU32::new(child_trust as u32),
            label: self.label.inherit(),
            restrictions: AtomicU64::new(self.restrictions.load(Ordering::Relaxed)),
            denied_count: AtomicU64::new(0),
            allowed_count: AtomicU64::new(0),
        }
    }

    /// Statistics snapshot.
    pub fn stats(&self) -> ContextStats {
        ContextStats {
            denied: self.denied_count.load(Ordering::Relaxed),
            allowed: self.allowed_count.load(Ordering::Relaxed),
        }
    }
}

/// Context statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct ContextStats {
    pub denied: u64,
    pub allowed: u64,
}
