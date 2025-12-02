// POSIX Process State
//
// Maintains POSIX-specific process information:
// - PID, PPID (parent process ID)
// - Current working directory (CWD)
// - Environment variables
// - Signal handlers
// - Exit status

use super::fd_table::FdTable;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// POSIX-specific process state
pub struct ProcessState {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub ppid: u32,
    /// Process group ID
    pub pgid: u32,
    /// Session ID
    pub sid: u32,

    /// User ID
    pub uid: u32,
    /// Group ID
    pub gid: u32,
    /// Effective user ID
    pub euid: u32,
    /// Effective group ID
    pub egid: u32,

    /// Current working directory
    pub cwd: String,

    /// Environment variables
    pub env: BTreeMap<String, String>,

    /// File descriptor table
    pub fd_table: FdTable,

    /// Signal handlers (signal number → handler function pointer)
    pub signal_handlers: BTreeMap<i32, SignalHandler>,

    /// Signal mask (blocked signals)
    pub signal_mask: u64,

    /// Exit status (if process has exited)
    pub exit_status: Option<i32>,
}

/// Signal handler type
#[derive(Debug, Clone, Copy)]
pub enum SignalHandler {
    /// Default handler
    Default,
    /// Ignore signal
    Ignore,
    /// Custom handler at address
    Custom(usize),
}

impl ProcessState {
    /// Create a new process state
    pub fn new(pid: u32, ppid: u32) -> Self {
        Self {
            pid,
            ppid,
            pgid: pid, // Process group = own PID by default
            sid: pid,  // Session ID = own PID by default
            uid: 1000, // Default user
            gid: 1000, // Default group
            euid: 1000,
            egid: 1000,
            cwd: String::from("/"),
            env: BTreeMap::new(),
            fd_table: FdTable::with_defaults(),
            signal_handlers: BTreeMap::new(),
            signal_mask: 0,
            exit_status: None,
        }
    }

    /// Clone process state for fork()
    pub fn clone_for_fork(&self, child_pid: u32) -> Self {
        Self {
            pid: child_pid,
            ppid: self.pid, // Parent = current process
            pgid: self.pgid,
            sid: self.sid,
            uid: self.uid,
            gid: self.gid,
            euid: self.euid,
            egid: self.egid,
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            fd_table: self.fd_table.clone_table(),
            signal_handlers: self.signal_handlers.clone(),
            signal_mask: self.signal_mask,
            exit_status: None,
        }
    }

    /// Set environment variable
    pub fn setenv(&mut self, key: String, value: String) {
        self.env.insert(key, value);
    }

    /// Get environment variable
    pub fn getenv(&self, key: &str) -> Option<&String> {
        self.env.get(key)
    }

    /// Unset environment variable
    pub fn unsetenv(&mut self, key: &str) {
        self.env.remove(key);
    }

    /// Set current working directory
    pub fn chdir(&mut self, path: String) {
        self.cwd = path;
    }

    /// Set signal handler
    pub fn set_signal_handler(&mut self, signal: i32, handler: SignalHandler) {
        self.signal_handlers.insert(signal, handler);
    }

    /// Get signal handler
    pub fn get_signal_handler(&self, signal: i32) -> SignalHandler {
        self.signal_handlers
            .get(&signal)
            .copied()
            .unwrap_or(SignalHandler::Default)
    }

    /// Check if signal is blocked
    pub fn is_signal_blocked(&self, signal: i32) -> bool {
        if signal < 1 || signal > 64 {
            return false;
        }
        (self.signal_mask & (1 << (signal - 1))) != 0
    }

    /// Block a signal
    pub fn block_signal(&mut self, signal: i32) {
        if signal >= 1 && signal <= 64 {
            self.signal_mask |= 1 << (signal - 1);
        }
    }

    /// Unblock a signal
    pub fn unblock_signal(&mut self, signal: i32) {
        if signal >= 1 && signal <= 64 {
            self.signal_mask &= !(1 << (signal - 1));
        }
    }
}

use alloc::sync::Arc;
/// Global process state table (PID → ProcessState)
use spin::RwLock;

static PROCESS_STATES: RwLock<BTreeMap<u32, Arc<RwLock<ProcessState>>>> =
    RwLock::new(BTreeMap::new());

/// Register a new process
pub fn register_process(state: ProcessState) {
    let pid = state.pid;
    PROCESS_STATES
        .write()
        .insert(pid, Arc::new(RwLock::new(state)));
}

/// Get process state by PID
pub fn get_process_state(pid: u32) -> Option<Arc<RwLock<ProcessState>>> {
    PROCESS_STATES.read().get(&pid).cloned()
}

/// Get current process state
pub fn current_process_state() -> Option<Arc<RwLock<ProcessState>>> {
    // TODO: Get current PID from scheduler
    let current_pid = 1; // Placeholder
    get_process_state(current_pid)
}

/// Remove process state (on exit)
pub fn unregister_process(pid: u32) {
    PROCESS_STATES.write().remove(&pid);
}
