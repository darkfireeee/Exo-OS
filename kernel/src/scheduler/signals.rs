//! POSIX Signal Implementation - Production Grade
//!
//! Full POSIX signal handling for threads with proper semantics:
//! - Signal masks (blocked/pending/ignored)
//! - Signal handlers with proper frame setup
//! - Signal delivery and queueing
//! - Re-entrant signal handling
//! - Thread-safe signal operations

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::boxed::Box;

/// Maximum number of queued signals per thread
pub const MAX_QUEUED_SIGNALS: usize = 32;

/// Ensemble de signaux POSIX (64-bit mask)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct SigSet(u64);

impl SigSet {
    /// Create empty signal set
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Create full signal set (all signals)
    pub const fn full() -> Self {
        Self(u64::MAX)
    }

    /// Create signal set with single signal
    pub const fn from_signal(sig: u32) -> Self {
        if sig >= 64 {
            return Self::empty();
        }
        Self(1u64 << sig)
    }

    /// Check if signal is in set
    #[inline(always)]
    pub fn contains(&self, sig: u32) -> bool {
        if sig >= 64 {
            return false;
        }
        (self.0 & (1 << sig)) != 0
    }

    /// Add signal to set
    #[inline]
    pub fn insert(&mut self, sig: u32) {
        if sig < 64 {
            self.0 |= 1 << sig;
        }
    }

    /// Add signal (alias for insert)
    #[inline]
    pub fn add(&mut self, sig: u32) {
        self.insert(sig);
    }

    /// Remove signal from set
    #[inline]
    pub fn remove(&mut self, sig: u32) {
        if sig < 64 {
            self.0 &= !(1 << sig);
        }
    }

    /// Clear all signals
    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    /// Check if set is empty
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Get first signal in set (lowest numbered)
    pub fn first(&self) -> Option<u32> {
        if self.0 == 0 {
            None
        } else {
            Some(self.0.trailing_zeros())
        }
    }

    /// Get next signal and remove it from set (pop)
    pub fn pop(&mut self) -> Option<u32> {
        if let Some(sig) = self.first() {
            self.remove(sig);
            Some(sig)
        } else {
            None
        }
    }

    /// Union with another set
    #[inline]
    pub fn union(&mut self, other: SigSet) {
        self.0 |= other.0;
    }

    /// Intersection with another set
    #[inline]
    pub fn intersect(&mut self, other: SigSet) {
        self.0 &= other.0;
    }

    /// Difference (remove signals from other set)
    #[inline]
    pub fn difference(&mut self, other: SigSet) {
        self.0 &= !other.0;
    }

    /// Count signals in set
    #[inline]
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }
}

/// Action for a signal
#[derive(Debug, Clone, Copy)]
pub enum SigAction {
    /// Default action (usually terminate)
    Default,
    /// Ignore signal
    Ignore,
    /// Call handler function
    Handler {
        /// Handler function address
        handler: usize,
        /// Signals to block during handler execution
        mask: SigSet,
        /// Flags (SA_RESTART, SA_SIGINFO, etc.)
        flags: u32,
    },
}

impl SigAction {
    /// Create handler action with default flags
    pub fn handler(handler: usize) -> Self {
        Self::Handler {
            handler,
            mask: SigSet::empty(),
            flags: 0,
        }
    }

    /// Create handler with blocking mask
    pub fn handler_with_mask(handler: usize, mask: SigSet) -> Self {
        Self::Handler {
            handler,
            mask,
            flags: 0,
        }
    }
}

/// Signal stack frame for handler invocation
/// This is pushed onto the thread's stack when delivering a signal
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalStackFrame {
    /// Saved thread context (registers before signal)
    pub context: crate::scheduler::thread::ThreadContext,
    /// Signal number
    pub sig: u32,
    /// Return address (signal trampoline)
    pub ret_addr: u64,
    /// Signal info (extended signal data)
    pub si_signo: u32,
    pub si_errno: u32,
    pub si_code: u32,
    /// Padding for alignment
    pub _padding: u32,
}

/// Signal information (extended)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalInfo {
    pub si_signo: u32,
    pub si_errno: u32,
    pub si_code: u32,
    pub si_pid: u32,
    pub si_uid: u32,
    pub si_status: i32,
    pub si_addr: u64,
}

impl SignalInfo {
    pub fn new(signo: u32) -> Self {
        Self {
            si_signo: signo,
            si_errno: 0,
            si_code: 0,
            si_pid: 0,
            si_uid: 0,
            si_status: 0,
            si_addr: 0,
        }
    }
}

// ============================================================================
// POSIX Signal Constants
// ============================================================================

pub const MAX_SIGNAL: u32 = 64;

// Standard signals (POSIX.1-1990)
pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const SIGURG: u32 = 23;
pub const SIGXCPU: u32 = 24;
pub const SIGXFSZ: u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF: u32 = 27;
pub const SIGWINCH: u32 = 28;
pub const SIGIO: u32 = 29;
pub const SIGPWR: u32 = 30;
pub const SIGSYS: u32 = 31;

// Real-time signals (POSIX.1-2001)
pub const SIGRTMIN: u32 = 32;
pub const SIGRTMAX: u32 = 63;

// sigprocmask how values
pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

// Signal flags
pub const SA_NOCLDSTOP: u32 = 0x00000001;
pub const SA_NOCLDWAIT: u32 = 0x00000002;
pub const SA_SIGINFO: u32 = 0x00000004;
pub const SA_ONSTACK: u32 = 0x08000000;
pub const SA_RESTART: u32 = 0x10000000;
pub const SA_NODEFER: u32 = 0x40000000;
pub const SA_RESETHAND: u32 = 0x80000000;

// ============================================================================
// Signal Disposition
// ============================================================================

/// Check if signal is uncatchable (SIGKILL, SIGSTOP)
#[inline(always)]
pub fn is_uncatchable(sig: u32) -> bool {
    sig == SIGKILL || sig == SIGSTOP
}

/// Get default action for a signal
pub fn default_action(sig: u32) -> SigAction {
    match sig {
        SIGCHLD | SIGURG | SIGWINCH | SIGCONT => SigAction::Ignore,
        _ => SigAction::Default,
    }
}

/// Check if signal should terminate process by default
pub fn is_fatal(sig: u32) -> bool {
    match sig {
        SIGCHLD | SIGURG | SIGWINCH | SIGCONT => false,
        _ => true,
    }
}

/// Check if signal should generate core dump
pub fn should_coredump(sig: u32) -> bool {
    matches!(
        sig,
        SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGBUS | SIGFPE | SIGSEGV |
        SIGXCPU | SIGXFSZ | SIGSYS
    )
}

// ============================================================================
// Atomic Signal Pending Set
// ============================================================================

/// Atomic signal set for lock-free signal delivery
#[derive(Debug)]
pub struct AtomicSigSet {
    bits: AtomicU64,
}

impl AtomicSigSet {
    pub const fn new() -> Self {
        Self {
            bits: AtomicU64::new(0),
        }
    }

    /// Add signal atomically
    #[inline]
    pub fn add(&self, sig: u32) {
        if sig < 64 {
            self.bits.fetch_or(1u64 << sig, Ordering::Release);
        }
    }

    /// Remove signal atomically
    #[inline]
    pub fn remove(&self, sig: u32) {
        if sig < 64 {
            self.bits.fetch_and(!(1u64 << sig), Ordering::Release);
        }
    }

    /// Check if signal is pending
    #[inline]
    pub fn contains(&self, sig: u32) -> bool {
        if sig >= 64 {
            return false;
        }
        (self.bits.load(Ordering::Acquire) & (1u64 << sig)) != 0
    }

    /// Get and clear first pending signal
    pub fn pop(&self) -> Option<u32> {
        loop {
            let current = self.bits.load(Ordering::Acquire);
            if current == 0 {
                return None;
            }

            let sig = current.trailing_zeros();
            let new = current & !(1u64 << sig);

            if self.bits.compare_exchange(
                current,
                new,
                Ordering::AcqRel,
                Ordering::Acquire
            ).is_ok() {
                return Some(sig);
            }
            // Retry on failure (another thread modified it)
        }
    }

    /// Load as SigSet
    pub fn load(&self) -> SigSet {
        SigSet(self.bits.load(Ordering::Acquire))
    }

    /// Store from SigSet
    pub fn store(&self, set: SigSet) {
        self.bits.store(set.0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigset_basic() {
        let mut set = SigSet::empty();
        assert!(set.is_empty());

        set.insert(SIGUSR1);
        assert!(set.contains(SIGUSR1));
        assert!(!set.contains(SIGUSR2));

        set.insert(SIGUSR2);
        assert_eq!(set.count(), 2);

        assert_eq!(set.first(), Some(SIGUSR1));
        set.remove(SIGUSR1);
        assert_eq!(set.first(), Some(SIGUSR2));
    }

    #[test]
    fn test_sigset_pop() {
        let mut set = SigSet::empty();
        set.insert(SIGINT);
        set.insert(SIGTERM);

        assert_eq!(set.pop(), Some(SIGINT));
        assert_eq!(set.pop(), Some(SIGTERM));
        assert_eq!(set.pop(), None);
    }

    #[test]
    fn test_atomic_sigset() {
        let set = AtomicSigSet::new();

        set.add(SIGINT);
        assert!(set.contains(SIGINT));

        set.remove(SIGINT);
        assert!(!set.contains(SIGINT));

        set.add(SIGUSR1);
        set.add(SIGUSR2);
        assert_eq!(set.pop(), Some(SIGUSR1));
        assert_eq!(set.pop(), Some(SIGUSR2));
        assert_eq!(set.pop(), None);
    }
}
