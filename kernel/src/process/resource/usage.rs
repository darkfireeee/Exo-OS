// kernel/src/process/resource/usage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Resource accounting (getrusage) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

/// Target for getrusage(2).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum RUsageWho {
    /// Current process.
    RSelf = 0,
    /// Terminated child processes.
    RChildren = -1,
    /// Current thread.
    RThread = 1,
}

/// Resource statistics (POSIX struct rusage).
#[repr(C)]
pub struct RUsage {
    /// User CPU time (µs).
    pub utime_us: AtomicU64,
    /// System CPU time (µs).
    pub stime_us: AtomicU64,
    /// Maximum memory usage (RSS in pages).
    pub maxrss_pages: AtomicU64,
    /// Minor page faults.
    pub minor_faults: AtomicU64,
    /// Major page faults.
    pub major_faults: AtomicU64,
    /// Voluntary context switches.
    pub vol_ctx_switches: AtomicU64,
    /// Involuntary context switches.
    pub invol_ctx_switches: AtomicU64,
    /// Number of syscalls.
    pub syscalls: AtomicU64,
    /// Number of read() calls.
    pub inblock: AtomicU64,
    /// Number of write() calls.
    pub outblock: AtomicU64,
    /// Signals received.
    pub signals_received: AtomicU64,
    /// Number of IPC messages sent.
    pub msgsnd: AtomicU64,
    /// Number of IPC messages received.
    pub msgrcv: AtomicU64,
}

impl Default for RUsage {
    fn default() -> Self {
        Self::new()
    }
}

impl RUsage {
    pub const fn new() -> Self {
        Self {
            utime_us: AtomicU64::new(0),
            stime_us: AtomicU64::new(0),
            maxrss_pages: AtomicU64::new(0),
            minor_faults: AtomicU64::new(0),
            major_faults: AtomicU64::new(0),
            vol_ctx_switches: AtomicU64::new(0),
            invol_ctx_switches: AtomicU64::new(0),
            syscalls: AtomicU64::new(0),
            inblock: AtomicU64::new(0),
            outblock: AtomicU64::new(0),
            signals_received: AtomicU64::new(0),
            msgsnd: AtomicU64::new(0),
            msgrcv: AtomicU64::new(0),
        }
    }

    // Update methods are atomic wrappers:

    pub fn add_utime(&self, us: u64) {
        self.utime_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn add_stime(&self, us: u64) {
        self.stime_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn update_maxrss(&self, pages: u64) {
        let mut current = self.maxrss_pages.load(Ordering::Relaxed);
        while pages > current {
            match self.maxrss_pages.compare_exchange_weak(
                current,
                pages,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => current = v,
            }
        }
    }

    pub fn add_minor_fault(&self) {
        self.minor_faults.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_major_fault(&self) {
        self.major_faults.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_vol_ctx(&self) {
        self.vol_ctx_switches.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_invol_ctx(&self) {
        self.invol_ctx_switches.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_syscall(&self) {
        self.syscalls.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_signal(&self) {
        self.signals_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Accumulates statistics of a terminated child into child statistics.
    pub fn accumulate_child(&self, child: &RUsage) {
        self.utime_us
            .fetch_add(child.utime_us.load(Ordering::Relaxed), Ordering::Relaxed);
        self.stime_us
            .fetch_add(child.stime_us.load(Ordering::Relaxed), Ordering::Relaxed);
        self.minor_faults.fetch_add(
            child.minor_faults.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.major_faults.fetch_add(
            child.major_faults.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    /// Copies snapshot to a C-style buffer (for getrusage syscall).
    pub fn snapshot(&self) -> RUsageSnapshot {
        RUsageSnapshot {
            utime_us: self.utime_us.load(Ordering::Relaxed),
            stime_us: self.stime_us.load(Ordering::Relaxed),
            maxrss_pages: self.maxrss_pages.load(Ordering::Relaxed),
            minor_faults: self.minor_faults.load(Ordering::Relaxed),
            major_faults: self.major_faults.load(Ordering::Relaxed),
            vol_ctx_switches: self.vol_ctx_switches.load(Ordering::Relaxed),
            invol_ctx_switches: self.invol_ctx_switches.load(Ordering::Relaxed),
            syscalls: self.syscalls.load(Ordering::Relaxed),
            inblock: self.inblock.load(Ordering::Relaxed),
            outblock: self.outblock.load(Ordering::Relaxed),
            signals_received: self.signals_received.load(Ordering::Relaxed),
            msgsnd: self.msgsnd.load(Ordering::Relaxed),
            msgrcv: self.msgrcv.load(Ordering::Relaxed),
        }
    }
}

/// Non-atomic snapshot of RUsage (for copying to userland).
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct RUsageSnapshot {
    pub utime_us: u64,
    pub stime_us: u64,
    pub maxrss_pages: u64,
    pub minor_faults: u64,
    pub major_faults: u64,
    pub vol_ctx_switches: u64,
    pub invol_ctx_switches: u64,
    pub syscalls: u64,
    pub inblock: u64,
    pub outblock: u64,
    pub signals_received: u64,
    pub msgsnd: u64,
    pub msgrcv: u64,
}
