// kernel/src/security/isolation/sandbox.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Sandbox — Per-process syscall filtering policy (Seccomp style)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • SandboxPolicy: allowed syscalls bitmap (256 syscalls, 4 × u64)
//   • DEFAULT_DENY: every policy starts in deny-all
//   • Actions per syscall: Allow, Deny, Errno(n), Kill
//   • Integration: the syscall handler checks policy prior to dispatch
//   • Instrumentation: denial/allow counters per process
//
// RULE SAND-01: SandboxPolicy can ONLY reduce rights (never increase).
// RULE SAND-02: A Sandboxed process cannot modify its own policy.
// RULE SAND-03: Inheritance: child receives a policy ⊆ parent.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Exo-OS syscall numbers (representative subset)
// ─────────────────────────────────────────────────────────────────────────────

pub mod syscall_nr {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const OPEN: usize = 2;
    pub const CLOSE: usize = 3;
    pub const STAT: usize = 4;
    pub const FSTAT: usize = 5;
    pub const LSTAT: usize = 6;
    pub const POLL: usize = 7;
    pub const LSEEK: usize = 8;
    pub const MMAP: usize = 9;
    pub const MPROTECT: usize = 10;
    pub const MUNMAP: usize = 11;
    pub const BRK: usize = 12;
    pub const RT_SIGACTION: usize = 13;
    pub const FORK: usize = 57;
    pub const EXECVE: usize = 59;
    pub const EXIT: usize = 60;
    pub const WAIT4: usize = 61;
    pub const KILL: usize = 62;
    pub const SOCKET: usize = 41;
    pub const CONNECT: usize = 42;
    pub const BIND: usize = 49;
    pub const LISTEN: usize = 50;
    pub const ACCEPT: usize = 43;
    pub const SEND: usize = 44;
    pub const RECV: usize = 45;
    pub const IOCTL: usize = 16;
    pub const PRCTL: usize = 157;
    pub const CLONE: usize = 56;
    pub const PTRACE: usize = 101;
    pub const MADVISE: usize = 28;
    pub const FUTEX: usize = 202;
    pub const GETPID: usize = 39;
    pub const GETTID: usize = 186;
    pub const GETTIMEOFDAY: usize = 96;
    pub const NANOSLEEP: usize = 35;
    pub const PIPE: usize = 22;
    pub const DUP: usize = 32;
    pub const DUP2: usize = 33;
    pub const GETUID: usize = 102;
    pub const GETGID: usize = 104;

    pub const MAX_SYSCALL: usize = 256;
}

// ─────────────────────────────────────────────────────────────────────────────
// SandboxAction — action taken for a filtered syscall
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxAction {
    /// Allow the syscall through.
    Allow = 0,
    /// Return EPERM (1).
    DenyEperm = 1,
    /// Return ENOSYS (2).
    DenyEnosys = 2,
    /// Kill the process (SIGKILL).
    Kill = 3,
    /// Log and return EPERM.
    LogAndDeny = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// SandboxPolicy — bitmap + actions for 256 syscalls
// ─────────────────────────────────────────────────────────────────────────────

/// Sandbox policy for a process.
///
/// Compact representation:
///   - `allowed_bitmap`: 4 × u64 = 256 bits — 1 if syscall is allowed
///   - `kill_bitmap`   : 4 × u64 — 1 if syscall → SIGKILL (priority over allowed)
///   - `log_bitmap`    : 4 × u64 — 1 if syscall → log + deny
#[derive(Clone)]
pub struct SandboxPolicy {
    allowed_bitmap: [u64; 4],
    kill_bitmap: [u64; 4],
    log_bitmap: [u64; 4],
    /// Number of recorded denials (instrumentation).
    denials: u64,
    /// Number of recorded allowances.
    allows: u64,
}

impl SandboxPolicy {
    /// Creates a deny-all policy (no syscall allowed).
    pub const fn deny_all() -> Self {
        Self {
            allowed_bitmap: [0u64; 4],
            kill_bitmap: [0u64; 4],
            log_bitmap: [0u64; 4],
            denials: 0,
            allows: 0,
        }
    }

    /// Creates an allow-all policy (all syscalls allowed).
    pub const fn allow_all() -> Self {
        Self {
            allowed_bitmap: [!0u64; 4],
            kill_bitmap: [0u64; 4],
            log_bitmap: [0u64; 4],
            denials: 0,
            allows: 0,
        }
    }

    /// Creates a minimal policy for a read-only process.
    pub fn read_only_minimal() -> Self {
        let mut p = Self::deny_all();
        for nr in [
            syscall_nr::READ,
            syscall_nr::FSTAT,
            syscall_nr::STAT,
            syscall_nr::LSEEK,
            syscall_nr::CLOSE,
            syscall_nr::MMAP,
            syscall_nr::MUNMAP,
            syscall_nr::EXIT,
            syscall_nr::GETPID,
            syscall_nr::GETTID,
            syscall_nr::GETTIMEOFDAY,
            syscall_nr::FUTEX,
            syscall_nr::NANOSLEEP,
            syscall_nr::MADVISE,
        ] {
            p.allow_syscall(nr);
        }
        p
    }

    /// Creates a policy for network processes (allow I/O + socket).
    pub fn network_io() -> Self {
        let mut p = Self::read_only_minimal();
        for nr in [
            syscall_nr::WRITE,
            syscall_nr::SOCKET,
            syscall_nr::CONNECT,
            syscall_nr::SEND,
            syscall_nr::RECV,
            syscall_nr::POLL,
            syscall_nr::PIPE,
            syscall_nr::DUP,
            syscall_nr::DUP2,
        ] {
            p.allow_syscall(nr);
        }
        p
    }

    /// Allows a syscall.
    pub fn allow_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.allowed_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Denies a syscall (removes from allowed list).
    pub fn deny_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.allowed_bitmap[nr / 64] &= !(1u64 << (nr % 64));
        }
    }

    /// Configures a syscall to kill the process if called.
    pub fn kill_on_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.deny_syscall(nr);
            self.kill_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Configures a syscall to log + deny.
    pub fn log_and_deny_syscall(&mut self, nr: usize) {
        if nr < syscall_nr::MAX_SYSCALL {
            self.deny_syscall(nr);
            self.log_bitmap[nr / 64] |= 1u64 << (nr % 64);
        }
    }

    /// Evaluates the action for a syscall number.
    pub fn evaluate(&mut self, nr: usize) -> SandboxAction {
        if nr >= syscall_nr::MAX_SYSCALL {
            self.denials += 1;
            return SandboxAction::DenyEnosys;
        }
        let word = nr / 64;
        let bit = 1u64 << (nr % 64);

        if self.kill_bitmap[word] & bit != 0 {
            self.denials += 1;
            return SandboxAction::Kill;
        }
        if self.log_bitmap[word] & bit != 0 {
            self.denials += 1;
            return SandboxAction::LogAndDeny;
        }
        if self.allowed_bitmap[word] & bit != 0 {
            self.allows += 1;
            return SandboxAction::Allow;
        }
        self.denials += 1;
        SandboxAction::DenyEperm
    }

    /// Evaluates without modifying counters (useful for preliminary checks).
    pub fn check(&self, nr: usize) -> SandboxAction {
        if nr >= syscall_nr::MAX_SYSCALL {
            return SandboxAction::DenyEnosys;
        }
        let word = nr / 64;
        let bit = 1u64 << (nr % 64);
        if self.kill_bitmap[word] & bit != 0 {
            return SandboxAction::Kill;
        }
        if self.log_bitmap[word] & bit != 0 {
            return SandboxAction::LogAndDeny;
        }
        if self.allowed_bitmap[word] & bit != 0 {
            return SandboxAction::Allow;
        }
        SandboxAction::DenyEperm
    }

    /// RULE SAND-03: Returns a child policy ⊆ self (bitmap AND).
    pub fn derive_child(&self) -> SandboxPolicy {
        // The child inherits exactly the same rights (subset guaranteed by AND)
        Self {
            allowed_bitmap: self.allowed_bitmap,
            kill_bitmap: self.kill_bitmap,
            log_bitmap: self.log_bitmap,
            denials: 0,
            allows: 0,
        }
    }

    /// Restricts current policy with `other` (intersection of allowed).
    pub fn intersect_with(&mut self, other: &SandboxPolicy) {
        for i in 0..4 {
            self.allowed_bitmap[i] &= other.allowed_bitmap[i];
            self.kill_bitmap[i] |= other.kill_bitmap[i];
            self.log_bitmap[i] |= other.log_bitmap[i];
        }
    }

    /// Policy statistics.
    pub fn stats(&self) -> (u64, u64) {
        (self.allows, self.denials)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global sandbox statistics
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_SANDBOX_DENIALS: AtomicU64 = AtomicU64::new(0);
static GLOBAL_SANDBOX_ALLOWS: AtomicU64 = AtomicU64::new(0);
static GLOBAL_SANDBOX_KILLS: AtomicU64 = AtomicU64::new(0);

/// Records a sandbox decision in global counters.
pub fn record_sandbox_decision(action: SandboxAction) {
    match action {
        SandboxAction::Allow => {
            GLOBAL_SANDBOX_ALLOWS.fetch_add(1, Ordering::Relaxed);
        }
        SandboxAction::Kill => {
            GLOBAL_SANDBOX_KILLS.fetch_add(1, Ordering::Relaxed);
            GLOBAL_SANDBOX_DENIALS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            GLOBAL_SANDBOX_DENIALS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SandboxGlobalStats {
    pub global_allows: u64,
    pub global_denials: u64,
    pub global_kills: u64,
}

pub fn sandbox_global_stats() -> SandboxGlobalStats {
    SandboxGlobalStats {
        global_allows: GLOBAL_SANDBOX_ALLOWS.load(Ordering::Relaxed),
        global_denials: GLOBAL_SANDBOX_DENIALS.load(Ordering::Relaxed),
        global_kills: GLOBAL_SANDBOX_KILLS.load(Ordering::Relaxed),
    }
}
