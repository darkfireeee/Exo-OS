//! Signal Daemon - POSIX Signal Handling
//!
//! Daemon that handles POSIX signal delivery and processing

use crate::ipc::message::Message;
use crate::posix_x::translation::signals_to_msgs::{signal_to_message, Signal};
use alloc::collections::BTreeMap;
use spin::RwLock;

/// Signal handler type
pub type SignalHandler = fn(Signal);

/// Signal disposition
#[derive(Clone, Copy)]
pub enum SignalDisposition {
    Default,
    Ignore,
    Handler(usize), // Function pointer address
}

/// Per-process signal state
pub struct SignalState {
    /// Signal handlers
    handlers: BTreeMap<Signal, SignalDisposition>,
    /// Pending signals
    pending: u64, // Bitmask of pending signals
    /// Blocked signals
    blocked: u64, // Bitmask of blocked signals
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
            pending: 0,
            blocked: 0,
        }
    }

    /// Set signal handler
    pub fn set_handler(&mut self, signal: Signal, disposition: SignalDisposition) {
        self.handlers.insert(signal, disposition);
    }

    /// Get signal handler
    pub fn get_handler(&self, signal: Signal) -> SignalDisposition {
        self.handlers
            .get(&signal)
            .copied()
            .unwrap_or(SignalDisposition::Default)
    }

    /// Mark signal as pending
    pub fn add_pending(&mut self, signal: Signal) {
        let bit = signal as u64;
        self.pending |= 1 << bit;
    }

    /// Check if signal is pending
    pub fn is_pending(&self, signal: Signal) -> bool {
        let bit = signal as u64;
        (self.pending & (1 << bit)) != 0
    }

    /// Clear pending signal
    pub fn clear_pending(&mut self, signal: Signal) {
        let bit = signal as u64;
        self.pending &= !(1 << bit);
    }

    /// Block signal
    pub fn block(&mut self, signal: Signal) {
        let bit = signal as u64;
        self.blocked |= 1 << bit;
    }

    /// Unblock signal
    pub fn unblock(&mut self, signal: Signal) {
        let bit = signal as u64;
        self.blocked &= !(1 << bit);
    }

    /// Check if signal is blocked
    pub fn is_blocked(&self, signal: Signal) -> bool {
        let bit = signal as u64;
        (self.blocked & (1 << bit)) != 0
    }
}

/// Global signal daemon
pub struct SignalDaemon {
    /// Per-process signal states
    process_states: RwLock<BTreeMap<u64, SignalState>>,
}

impl SignalDaemon {
    pub const fn new() -> Self {
        Self {
            process_states: RwLock::new(BTreeMap::new()),
        }
    }

    /// Register process
    pub fn register_process(&self, pid: u64) {
        let mut states = self.process_states.write();
        states.insert(pid, SignalState::new());
    }

    /// Unregister process
    pub fn unregister_process(&self, pid: u64) {
        let mut states = self.process_states.write();
        states.remove(&pid);
    }

    /// Send signal to process
    pub fn send_signal(&self, target_pid: u64, signal: Signal, sender_pid: u64) -> bool {
        // Check if signal is catchable
        if !signal.is_catchable() {
            // SIGKILL/SIGSTOP cannot be caught or ignored
            self.deliver_uncatchable_signal(target_pid, signal);
            return true;
        }

        let mut states = self.process_states.write();
        if let Some(state) = states.get_mut(&target_pid) {
            // Check if blocked
            if state.is_blocked(signal) {
                state.add_pending(signal);
                return true;
            }

            // Get handler
            match state.get_handler(signal) {
                SignalDisposition::Ignore => {
                    // Ignore signal
                    return true;
                }
                SignalDisposition::Default => {
                    // Use default action
                    self.default_signal_action(target_pid, signal);
                    return true;
                }
                SignalDisposition::Handler(addr) => {
                    // Queue for user-space handler
                    self.queue_signal_for_handler(target_pid, signal, addr);
                    return true;
                }
            }
        }

        false
    }

    fn deliver_uncatchable_signal(&self, pid: u64, signal: Signal) {
        match signal {
            Signal::SIGKILL => {
                log::info!("Process {} killed by SIGKILL", pid);
                // Terminate process immediately
            }
            Signal::SIGSTOP => {
                log::info!("Process {} stopped by SIGSTOP", pid);
                // Stop process
            }
            _ => {}
        }
    }

    fn default_signal_action(&self, pid: u64, signal: Signal) {
        use crate::posix_x::translation::signals_to_msgs::default_action;
        use crate::posix_x::translation::signals_to_msgs::SignalAction;

        match default_action(signal) {
            SignalAction::Terminate => {
                log::info!("Process {} terminated by {:?}", pid, signal);
            }
            SignalAction::CoreDump => {
                log::info!("Process {} core dumped by {:?}", pid, signal);
            }
            SignalAction::Stop => {
                log::info!("Process {} stopped by {:?}", pid, signal);
            }
            SignalAction::Ignore => {}
            SignalAction::Continue => {
                log::info!("Process {} continued", pid);
            }
        }
    }

    fn queue_signal_for_handler(&self, _pid: u64, _signal: Signal, _handler_addr: usize) {
        // Would queue signal for user-space handler delivery
        log::debug!("Queuing signal for user handler");
    }
}

/// Global signal daemon instance
pub static SIGNAL_DAEMON: SignalDaemon = SignalDaemon::new();

/// Initialize signal daemon
pub fn init() {
    log::debug!("Signal daemon initialized");
}

/// Send signal (convenience function)
pub fn send_signal(target_pid: u64, signal: Signal, sender_pid: u64) -> bool {
    SIGNAL_DAEMON.send_signal(target_pid, signal, sender_pid)
}
