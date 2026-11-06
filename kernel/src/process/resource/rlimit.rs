// kernel/src/process/resource/rlimit.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX resource limits (getrlimit/setrlimit) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

/// Infinite value (unlimited).
pub const RLIM_INFINITY: u64 = u64::MAX;

/// Limited resources (Linux x86_64).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum RLimitKind {
    CPU = 0,     // CPU time in seconds
    FSIZE = 1,   // Max file size
    DATA = 2,    // Max data segment
    STACK = 3,   // User stack size
    CORE = 4,    // Core dump size
    RSS = 5,     // Resident set size
    NPROC = 6,   // Number of processes
    NOFILE = 7,  // Number of open fds
    MEMLOCK = 8, // Locked memory
    AS = 9,      // Address space
    LOCKS = 10,  // File locks
    SIGPENDING = 11,
    MSGQUEUE = 12,
    NICE = 13,
    RTPRIO = 14,
    RTTIME = 15,
}

impl RLimitKind {
    pub const COUNT: usize = 16;

    pub fn from_u8(n: u8) -> Option<Self> {
        if n < 16 {
            // SAFETY: n < COUNT, all discriminants from 0 to 15 are valid.
            Some(unsafe { core::mem::transmute::<u8, RLimitKind>(n) })
        } else {
            None
        }
    }
}

/// Pair (soft, hard).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct RLimit {
    pub soft: u64,
    pub hard: u64,
}

impl RLimit {
    pub const UNLIMITED: Self = Self {
        soft: RLIM_INFINITY,
        hard: RLIM_INFINITY,
    };

    /// Default values for a user process.
    pub const DEFAULT_NOFILE: Self = Self {
        soft: 1024,
        hard: 4096,
    };
    pub const DEFAULT_STACK: Self = Self {
        soft: 8 * 1024 * 1024,
        hard: RLIM_INFINITY,
    };
    pub const DEFAULT_NPROC: Self = Self {
        soft: 32768,
        hard: 32768,
    };
    pub const DEFAULT_AS: Self = Self {
        soft: RLIM_INFINITY,
        hard: RLIM_INFINITY,
    };

    pub fn is_exceeded(&self, usage: u64) -> bool {
        self.soft != RLIM_INFINITY && usage > self.soft
    }
}

/// Limits table for a process.
#[derive(Clone)]
#[repr(C)]
pub struct RLimitTable {
    limits: [RLimit; RLimitKind::COUNT],
}

impl RLimitTable {
    pub fn new_default() -> Self {
        let mut limits = [RLimit::UNLIMITED; RLimitKind::COUNT];
        limits[RLimitKind::NOFILE as usize] = RLimit::DEFAULT_NOFILE;
        limits[RLimitKind::STACK as usize] = RLimit::DEFAULT_STACK;
        limits[RLimitKind::NPROC as usize] = RLimit::DEFAULT_NPROC;
        limits[RLimitKind::AS as usize] = RLimit::DEFAULT_AS;
        // RLIMIT_CORE = 0 by default (no core dump).
        limits[RLimitKind::CORE as usize] = RLimit {
            soft: 0,
            hard: RLIM_INFINITY,
        };
        Self { limits }
    }

    pub fn get(&self, kind: RLimitKind) -> RLimit {
        self.limits[kind as usize]
    }

    /// setrlimit(2): modifies a limit.
    /// POSIX rule: soft <= hard, and root can raise hard.
    pub fn set(&mut self, kind: RLimitKind, new: RLimit, is_root: bool) -> Result<(), RlimitError> {
        let current = self.limits[kind as usize];
        // POSIX checks
        if new.soft > new.hard {
            return Err(RlimitError::InvalidInput);
        }
        if !is_root && new.hard > current.hard {
            return Err(RlimitError::NotPermitted);
        }
        self.limits[kind as usize] = new;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlimitError {
    InvalidInput,
    NotPermitted,
    InvalidKind,
}
