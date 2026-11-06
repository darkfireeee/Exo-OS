// kernel/src/security/isolation/pledge.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Pledge — OpenBSD-style capability restrictions
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • PledgeSet: bitmask of "promises" (pledges) that the process commits to respect
//   • Once enabled, pledges can ONLY be reduced
//   • Integration with SandboxPolicy: pledges generate a SandboxPolicy
//   • Available pledges: stdio, rpath, wpath, cpath, tmppath,
//     inet, unix, dns, getpw, proc, exec, id, route, etc.
//
// RULE PLEDGE-01: A process can ONLY drop pledges, never add them.
// RULE PLEDGE-02: Violation of a pledge → immediate SIGKILL.
// RULE PLEDGE-03: The init process cannot call pledge().
// ═══════════════════════════════════════════════════════════════════════════════

use super::sandbox::{syscall_nr, SandboxPolicy};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Available pledges
// ─────────────────────────────────────────────────────────────────────────────

/// Pledge flags (u64 bitmask).
pub mod pledge_flags {
    /// Basic I/O operations: read, write, recvfrom, sendto, etc.
    pub const STDIO: u64 = 1 << 0;
    /// File read (open in read mode, stat, etc.).
    pub const RPATH: u64 = 1 << 1;
    /// File write (open in write mode).
    pub const WPATH: u64 = 1 << 2;
    /// File creation (creat, unlink, rename, etc.).
    pub const CPATH: u64 = 1 << 3;
    /// Access to /tmp.
    pub const TMPPATH: u64 = 1 << 4;
    /// TCP/UDP sockets.
    pub const INET: u64 = 1 << 5;
    /// Unix Domain sockets.
    pub const UNIX: u64 = 1 << 6;
    /// DNS resolution.
    pub const DNS: u64 = 1 << 7;
    /// Read /etc/passwd and /etc/group.
    pub const GETPW: u64 = 1 << 8;
    /// Child process creation.
    pub const PROC: u64 = 1 << 9;
    /// execve().
    pub const EXEC: u64 = 1 << 10;
    /// setuid/setgid/getuid/getgid/etc.
    pub const ID: u64 = 1 << 11;
    /// Routing table management.
    pub const ROUTE: u64 = 1 << 12;
    /// Shared memory.
    pub const SHM: u64 = 1 << 13;
    /// Signal management (sigaction, sigprocmask, etc.).
    pub const SIGNAL: u64 = 1 << 14;
    /// ioctl calls limited to terminals.
    pub const TTY: u64 = 1 << 15;
    /// Futex operations (userspace mutexes).
    pub const FUTEX: u64 = 1 << 16;
    /// All privileges (no restrictions).
    pub const UNRESTRICTED: u64 = !0u64;
}

// ─────────────────────────────────────────────────────────────────────────────
// PledgeSet — process pledge state
// ─────────────────────────────────────────────────────────────────────────────

/// Pledge state of a process.
#[derive(Clone, Copy)]
pub struct PledgeSet {
    /// Active pledges (bitmask).
    active: u64,
    /// Active pledges at initial activation (non-modifiable).
    initial: u64,
    /// Pledge enabled (true = pledge() was called).
    enabled: bool,
    /// Violations counter.
    violations: u64,
}

impl PledgeSet {
    /// Initial state: no pledge (not enabled = UNRESTRICTED).
    pub const fn new() -> Self {
        Self {
            active: pledge_flags::UNRESTRICTED,
            initial: pledge_flags::UNRESTRICTED,
            enabled: false,
            violations: 0,
        }
    }

    /// Initial state with restricted pledges.
    pub const fn restricted(flags: u64) -> Self {
        Self {
            active: flags,
            initial: flags,
            enabled: true,
            violations: 0,
        }
    }

    /// Enables pledges. Once enabled, can only be restricted.
    ///
    /// RULE PLEDGE-01: `flags` must be ⊆ `self.active`.
    pub fn pledge(&mut self, flags: u64) -> Result<(), PledgeError> {
        if self.enabled {
            // Verify that new flags are a subset of active ones
            if flags & !self.active != 0 {
                return Err(PledgeError::CannotExpand);
            }
        }
        self.active = flags;
        self.enabled = true;
        Ok(())
    }

    /// Checks if a pledge flag is active.
    pub fn has(&self, flag: u64) -> bool {
        if !self.enabled {
            return true;
        } // Not enabled = all permitted
        self.active & flag != 0
    }

    /// Generates the SandboxPolicy corresponding to active pledges.
    pub fn to_sandbox_policy(&self) -> SandboxPolicy {
        if !self.enabled {
            return SandboxPolicy::allow_all();
        }
        let mut policy = SandboxPolicy::deny_all();

        // STDIO: basic I/O operations
        if self.has(pledge_flags::STDIO) {
            for nr in [
                syscall_nr::READ,
                syscall_nr::WRITE,
                syscall_nr::CLOSE,
                syscall_nr::FSTAT,
                syscall_nr::LSEEK,
                syscall_nr::MMAP,
                syscall_nr::MUNMAP,
                syscall_nr::MPROTECT,
                syscall_nr::MADVISE,
                syscall_nr::GETPID,
                syscall_nr::GETTID,
                syscall_nr::GETTIMEOFDAY,
                syscall_nr::NANOSLEEP,
                syscall_nr::FUTEX,
                syscall_nr::PIPE,
                syscall_nr::DUP,
                syscall_nr::DUP2,
            ] {
                policy.allow_syscall(nr);
            }
        }

        // RPATH: file read
        if self.has(pledge_flags::RPATH) {
            for nr in [syscall_nr::OPEN, syscall_nr::STAT, syscall_nr::LSTAT] {
                policy.allow_syscall(nr);
            }
        }

        // WPATH: file write
        if self.has(pledge_flags::WPATH) {
            policy.allow_syscall(syscall_nr::OPEN);
        }

        // PROC: process creation
        if self.has(pledge_flags::PROC) {
            for nr in [syscall_nr::FORK, syscall_nr::CLONE, syscall_nr::WAIT4] {
                policy.allow_syscall(nr);
            }
        }

        // EXEC: execve
        if self.has(pledge_flags::EXEC) {
            policy.allow_syscall(syscall_nr::EXECVE);
        }

        // INET: network sockets
        if self.has(pledge_flags::INET) {
            for nr in [
                syscall_nr::SOCKET,
                syscall_nr::CONNECT,
                syscall_nr::BIND,
                syscall_nr::LISTEN,
                syscall_nr::ACCEPT,
                syscall_nr::SEND,
                syscall_nr::RECV,
            ] {
                policy.allow_syscall(nr);
            }
        }

        // ID: uid/gid
        if self.has(pledge_flags::ID) {
            for nr in [syscall_nr::GETUID, syscall_nr::GETGID] {
                policy.allow_syscall(nr);
            }
        }

        // EXIT is always permitted
        policy.allow_syscall(syscall_nr::EXIT);

        // SIGNAL
        if self.has(pledge_flags::SIGNAL) {
            policy.allow_syscall(syscall_nr::RT_SIGACTION);
        }

        // TTY: limited terminal ioctl
        if self.has(pledge_flags::TTY) {
            policy.allow_syscall(syscall_nr::IOCTL);
        }

        // FUTEX: userspace mutex
        if self.has(pledge_flags::FUTEX) {
            policy.allow_syscall(syscall_nr::FUTEX);
        }

        // PTRACE is always forbidden in pledge mode (RULE PLEDGE-02)
        policy.kill_on_syscall(syscall_nr::PTRACE);

        policy
    }

    /// Checks if pledge is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns active flags.
    pub fn active_flags(&self) -> u64 {
        self.active
    }

    /// Records a pledge violation.
    pub fn record_violation(&mut self) {
        self.violations += 1;
        GLOBAL_PLEDGE_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the number of violations.
    pub fn violations(&self) -> u64 {
        self.violations
    }

    /// Derives a child PledgeSet ⊆ self.
    pub fn derive_child(&self) -> PledgeSet {
        PledgeSet {
            active: self.active,
            initial: self.initial,
            enabled: self.enabled,
            violations: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pledge errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum PledgeError {
    /// Attempt to expand pledges (RULE PLEDGE-01).
    CannotExpand,
    /// Pledge called by init process (RULE PLEDGE-03).
    InitProcessCannotPledge,
    /// Invalid pledge flags.
    InvalidFlags,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global statistics
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_PLEDGE_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

pub fn global_pledge_violations() -> u64 {
    GLOBAL_PLEDGE_VIOLATIONS.load(Ordering::Relaxed)
}
