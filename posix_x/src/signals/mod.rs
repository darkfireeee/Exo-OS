//! # Signal Handling for POSIX-X
//!
//! Translates POSIX signals to Exo-OS native event system.

use crate::PosixXError;

/// Signal numbers (POSIX)
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGSTKFLT = 16,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
    SIGURG = 23,
    SIGXCPU = 24,
    SIGXFSZ = 25,
    SIGVTALRM = 26,
    SIGPROF = 27,
    SIGWINCH = 28,
    SIGIO = 29,
    SIGPWR = 30,
    SIGSYS = 31,
}

/// Signal action
#[derive(Debug, Clone, Copy)]
pub enum SignalAction {
    /// Default action
    Default,
    /// Ignore signal
    Ignore,
    /// Custom handler
    Handler(u64),
}

/// Signal handler state
pub struct SignalHandlers {
    /// Handlers for each signal (indexed by signal number)
    handlers: [SignalAction; 32],
    /// Blocked signals mask
    blocked: u64,
    /// Pending signals mask
    pending: u64,
}

impl SignalHandlers {
    /// Create new signal handlers with defaults
    pub fn new() -> Self {
        Self {
            handlers: [SignalAction::Default; 32],
            blocked: 0,
            pending: 0,
        }
    }

    /// Set signal handler
    pub fn set_handler(&mut self, sig: Signal, action: SignalAction) -> Result<SignalAction, PosixXError> {
        let sig_num = sig as usize;
        if sig_num >= 32 {
            return Err(PosixXError::InvalidArgument("Invalid signal number".into()));
        }

        // SIGKILL and SIGSTOP cannot be caught or ignored
        if matches!(sig, Signal::SIGKILL | Signal::SIGSTOP) && !matches!(action, SignalAction::Default) {
            return Err(PosixXError::InvalidArgument(
                "Cannot catch or ignore SIGKILL/SIGSTOP".into(),
            ));
        }

        let old = self.handlers[sig_num];
        self.handlers[sig_num] = action;
        Ok(old)
    }

    /// Get signal handler
    pub fn get_handler(&self, sig: Signal) -> SignalAction {
        let sig_num = sig as usize;
        if sig_num < 32 {
            self.handlers[sig_num]
        } else {
            SignalAction::Default
        }
    }

    /// Block signals
    pub fn block_signals(&mut self, mask: u64) {
        self.blocked |= mask;
    }

    /// Unblock signals
    pub fn unblock_signals(&mut self, mask: u64) {
        self.blocked &= !mask;
    }

    /// Set pending signal
    pub fn set_pending(&mut self, sig: Signal) {
        let sig_num = sig as usize;
        if sig_num < 32 {
            self.pending |= 1 << sig_num;
        }
    }

    /// Check if signal is pending and not blocked
    pub fn is_deliverable(&self, sig: Signal) -> bool {
        let sig_num = sig as usize;
        if sig_num >= 32 {
            return false;
        }
        let mask = 1u64 << sig_num;
        (self.pending & mask) != 0 && (self.blocked & mask) == 0
    }

    /// Clear pending signal
    pub fn clear_pending(&mut self, sig: Signal) {
        let sig_num = sig as usize;
        if sig_num < 32 {
            self.pending &= !(1 << sig_num);
        }
    }
}

impl Default for SignalHandlers {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize signal subsystem
pub fn init() -> Result<(), PosixXError> {
    log::debug!("Signal subsystem initialized");
    Ok(())
}
