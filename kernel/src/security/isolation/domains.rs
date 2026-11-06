// kernel/src/security/isolation/domains.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Security Domains — Execution domain isolation
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • Each thread/process belongs to ONE single security domain
//   • Transitions are strictly unidirectional (Kernel→Driver→User→Sandbox)
//   • Cross-domain accesses are filtered by the zero_trust policy
//   • Instrumentation: atomic counters per transition
//
// Domains:
//   Kernel(0)  : kernel code — ring 0, full access
//   Driver(1)  : certified drivers — isolated ring 0, limited MMIO/DMA access
//   User(2)    : standard user processes — ring 3
//   Sandbox(3) : sandboxed processes — ring 3, filtered syscalls
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SecurityDomain — domain identifier
// ─────────────────────────────────────────────────────────────────────────────

/// Security domain of a thread or process.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityDomain {
    /// Pure kernel — ring 0, no restrictions.
    Kernel = 0,
    /// Certified driver — ring 0 with IOMMU/MMIO policy.
    Driver = 1,
    /// Standard user process — ring 3.
    User = 2,
    /// Sandboxed process — ring 3, pledge() + syscall filter.
    Sandbox = 3,
}

impl SecurityDomain {
    /// Returns the numerical trust level of the domain.
    pub fn trust_level(&self) -> u32 {
        match self {
            SecurityDomain::Kernel => 100,
            SecurityDomain::Driver => 75,
            SecurityDomain::User => 50,
            SecurityDomain::Sandbox => 10,
        }
    }

    /// Checks if this domain can access the `target` domain.
    ///
    /// Rule: a domain with lower trust CANNOT access
    /// a domain with higher trust without an explicit gate.
    pub fn can_access(&self, target: SecurityDomain) -> bool {
        self.trust_level() >= target.trust_level()
    }

    /// Checks if a transition from `self` → `next` is allowed.
    ///
    /// Transitions allowed via gate:
    ///   User → Kernel (syscall), User → Driver (ioctl via /dev)
    ///   Driver → Kernel (driver_call)
    ///   Sandbox → User is FORBIDDEN (strict isolation)
    pub fn is_transition_allowed(&self, next: SecurityDomain) -> bool {
        match (self, next) {
            // Kernel can go anywhere
            (SecurityDomain::Kernel, _) => true,
            // Driver → Kernel via driver_call gate
            (SecurityDomain::Driver, SecurityDomain::Kernel) => true,
            // User → Kernel via syscall (normal)
            (SecurityDomain::User, SecurityDomain::Kernel) => true,
            // Sandbox → Kernel via syscall (filtered by sandbox policy)
            (SecurityDomain::Sandbox, SecurityDomain::Kernel) => true,
            // Any other transition is rejected
            _ => false,
        }
    }

    /// Encodes the domain into a u8.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Decodes from a u8.
    pub fn from_u8(v: u8) -> Option<SecurityDomain> {
        match v {
            0 => Some(SecurityDomain::Kernel),
            1 => Some(SecurityDomain::Driver),
            2 => Some(SecurityDomain::User),
            3 => Some(SecurityDomain::Sandbox),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DomainContext — execution context of a thread within a domain
// ─────────────────────────────────────────────────────────────────────────────

/// Domain context per thread — stored in the thread's TCB.
#[repr(C)]
pub struct DomainContext {
    /// Current domain.
    pub domain: SecurityDomain,
    /// Origin domain (before the last transition).
    pub previous_domain: SecurityDomain,
    /// Owner thread PID.
    pub owner_pid: u32,
    /// Owner thread TID.
    pub owner_tid: u32,
    /// Number of domain transitions performed.
    pub transition_count: AtomicU32,
    /// Additional flags (bitmask).
    pub flags: AtomicU64,
}

/// Domain context flags.
pub mod domain_flags {
    /// Thread undergoing domain transition.
    pub const TRANSITIONING: u64 = 1 << 0;
    /// Thread is an internal kernel thread.
    pub const KERNEL_THREAD: u64 = 1 << 1;
    /// Thread was created via fork() from a Sandbox.
    pub const SANDBOX_ORIGIN: u64 = 1 << 2;
    /// Thread is in debug state.
    pub const UNDER_PTRACE: u64 = 1 << 3;
    /// Hardened memory isolation (PKRU enabled).
    pub const PKRU_ISOLATED: u64 = 1 << 4;
}

impl DomainContext {
    /// Creates a domain context for a kernel thread.
    pub fn new_kernel(pid: u32, tid: u32) -> Self {
        Self {
            domain: SecurityDomain::Kernel,
            previous_domain: SecurityDomain::Kernel,
            owner_pid: pid,
            owner_tid: tid,
            transition_count: AtomicU32::new(0),
            flags: AtomicU64::new(domain_flags::KERNEL_THREAD),
        }
    }

    /// Creates a domain context for a user process.
    pub fn new_user(pid: u32, tid: u32) -> Self {
        Self {
            domain: SecurityDomain::User,
            previous_domain: SecurityDomain::Kernel,
            owner_pid: pid,
            owner_tid: tid,
            transition_count: AtomicU32::new(0),
            flags: AtomicU64::new(0),
        }
    }

    /// Creates a domain context for a sandboxed process.
    pub fn new_sandbox(pid: u32, tid: u32) -> Self {
        Self {
            domain: SecurityDomain::Sandbox,
            previous_domain: SecurityDomain::User,
            owner_pid: pid,
            owner_tid: tid,
            transition_count: AtomicU32::new(0),
            flags: AtomicU64::new(domain_flags::SANDBOX_ORIGIN),
        }
    }

    /// Performs a transition to `new_domain`.
    /// Returns Err if the transition is not allowed.
    pub fn transition_to(&mut self, new_domain: SecurityDomain) -> Result<(), DomainError> {
        if !self.domain.is_transition_allowed(new_domain) {
            DOMAIN_STATS
                .forbidden_transitions
                .fetch_add(1, Ordering::Relaxed);
            return Err(DomainError::TransitionNotAllowed {
                from: self.domain,
                to: new_domain,
            });
        }
        self.flags
            .fetch_or(domain_flags::TRANSITIONING, Ordering::Release);
        self.previous_domain = self.domain;
        self.domain = new_domain;
        self.transition_count.fetch_add(1, Ordering::Relaxed);
        self.flags
            .fetch_and(!domain_flags::TRANSITIONING, Ordering::Release);
        DOMAIN_STATS
            .successful_transitions
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Returns to previous domain (return from syscall).
    pub fn return_to_previous(&mut self) {
        let prev = self.previous_domain;
        self.domain = prev;
        self.transition_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Checks if context is in Kernel domain.
    #[inline(always)]
    pub fn is_kernel(&self) -> bool {
        self.domain == SecurityDomain::Kernel
    }

    /// Checks if context is sandboxed.
    #[inline(always)]
    pub fn is_sandboxed(&self) -> bool {
        self.domain == SecurityDomain::Sandbox
    }

    /// Adds a flag.
    pub fn set_flag(&self, flag: u64) {
        self.flags.fetch_or(flag, Ordering::Relaxed);
    }

    /// Clears a flag.
    pub fn clear_flag(&self, flag: u64) {
        self.flags.fetch_and(!flag, Ordering::Relaxed);
    }

    /// Checks a flag.
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags.load(Ordering::Relaxed) & flag != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Domain errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum DomainError {
    /// Unauthorized domain transition.
    TransitionNotAllowed {
        from: SecurityDomain,
        to: SecurityDomain,
    },
    /// Cross-domain access denied.
    CrossDomainAccessDenied,
    /// Unknown domain.
    UnknownDomain,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global domain statistics
// ─────────────────────────────────────────────────────────────────────────────

struct DomainStats {
    successful_transitions: AtomicU64,
    forbidden_transitions: AtomicU64,
    cross_domain_denials: AtomicU64,
}

static DOMAIN_STATS: DomainStats = DomainStats {
    successful_transitions: AtomicU64::new(0),
    forbidden_transitions: AtomicU64::new(0),
    cross_domain_denials: AtomicU64::new(0),
};

/// Returns domain statistics as a snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DomainStatsSnapshot {
    pub successful_transitions: u64,
    pub forbidden_transitions: u64,
    pub cross_domain_denials: u64,
}

pub fn read_domain_stats() -> DomainStatsSnapshot {
    DomainStatsSnapshot {
        successful_transitions: DOMAIN_STATS.successful_transitions.load(Ordering::Relaxed),
        forbidden_transitions: DOMAIN_STATS.forbidden_transitions.load(Ordering::Relaxed),
        cross_domain_denials: DOMAIN_STATS.cross_domain_denials.load(Ordering::Relaxed),
    }
}
