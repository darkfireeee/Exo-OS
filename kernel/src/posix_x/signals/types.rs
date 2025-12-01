//! Signal types and constants - Phase 11
//!
//! POSIX signal infrastructure

/// Signal numbers (POSIX standard)
pub const SIGHUP: u32 = 1; // Hangup
pub const SIGINT: u32 = 2; // Interrupt (Ctrl+C)
pub const SIGQUIT: u32 = 3; // Quit (Ctrl+\)
pub const SIGILL: u32 = 4; // Illegal instruction
pub const SIGTRAP: u32 = 5; // Trace trap
pub const SIGABRT: u32 = 6; // Abort
pub const SIGBUS: u32 = 7; // Bus error
pub const SIGFPE: u32 = 8; // Floating point exception
pub const SIGKILL: u32 = 9; // Kill (uncatchable)
pub const SIGUSR1: u32 = 10; // User-defined 1
pub const SIGSEGV: u32 = 11; // Segmentation fault
pub const SIGUSR2: u32 = 12; // User-defined 2
pub const SIGPIPE: u32 = 13; // Broken pipe
pub const SIGALRM: u32 = 14; // Alarm clock
pub const SIGTERM: u32 = 15; // Termination
pub const SIGSTKFLT: u32 = 16; // Stack fault
pub const SIGCHLD: u32 = 17; // Child status changed
pub const SIGCONT: u32 = 18; // Continue
pub const SIGSTOP: u32 = 19; // Stop (uncatchable)
pub const SIGTSTP: u32 = 20; // Terminal stop
pub const SIGTTIN: u32 = 21; // Background read
pub const SIGTTOU: u32 = 22; // Background write

pub const MAX_SIGNAL: u32 = 64; // Support up to 64 signals

/// Signal set (bitmap for 64 signals)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SigSet {
    bits: u64,
}

impl SigSet {
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn full() -> Self {
        Self { bits: u64::MAX }
    }

    pub fn add(&mut self, sig: u32) {
        if sig > 0 && sig <= MAX_SIGNAL {
            self.bits |= 1 << (sig - 1);
        }
    }

    pub fn remove(&mut self, sig: u32) {
        if sig > 0 && sig <= MAX_SIGNAL {
            self.bits &= !(1 << (sig - 1));
        }
    }

    pub fn contains(&self, sig: u32) -> bool {
        if sig > 0 && sig <= MAX_SIGNAL {
            (self.bits & (1 << (sig - 1))) != 0
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    pub fn bits(&self) -> u64 {
        self.bits
    }
}

/// Signal action
#[derive(Debug, Clone, Copy)]
pub enum SigAction {
    Default,                                  // Default action
    Ignore,                                   // Ignore signal
    Handler { handler: usize, mask: SigSet }, // User handler + mask
}

impl Default for SigAction {
    fn default() -> Self {
        SigAction::Default
    }
}

/// Signal handler flags
pub const SA_NOCLDSTOP: u32 = 0x00000001;
pub const SA_NOCLDWAIT: u32 = 0x00000002;
pub const SA_SIGINFO: u32 = 0x00000004;
pub const SA_ONSTACK: u32 = 0x08000000;
pub const SA_RESTART: u32 = 0x10000000;
pub const SA_NODEFER: u32 = 0x40000000;
pub const SA_RESETHAND: u32 = 0x80000000;

/// sigprocmask how parameter
pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// Signal stack frame (pushed to user stack)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalStackFrame {
    /// Saved thread context (registers)
    pub context: crate::scheduler::ThreadContext,
    /// Signal number
    pub sig: u32,
    /// Return address (trampoline)
    pub ret_addr: u64,
}
