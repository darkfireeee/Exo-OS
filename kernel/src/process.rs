//! Process Management Stub
//!
//! Minimal process abstraction for scheduler integration.
//! Full implementation is in posix_x::core::process_state.

use alloc::string::String;
use crate::memory::VirtualAddress;

/// Process ID type
pub type Pid = u32;

/// Process state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessState {
    /// Process is ready to run
    Ready = 0,
    /// Process is running
    Running = 1,
    /// Process is blocked/sleeping
    Blocked = 2,
    /// Process is zombie (exited but not reaped)
    Zombie = 3,
}

/// Minimal process representation
///
/// This is a lightweight stub for scheduler integration.
/// Full POSIX process functionality is in `posix_x::core::process_state`.
#[derive(Debug)]
pub struct Process {
    /// Process ID
    pub pid: Pid,
    /// Parent process ID
    pub ppid: Pid,
    /// Process name
    pub name: String,
    /// Process state
    pub state: ProcessState,
    /// Page table root (CR3 value on x86_64)
    pub page_table_root: Option<VirtualAddress>,
    /// Address space (stub for compatibility)
    pub address_space: Option<VirtualAddress>,
}

impl Process {
    /// Create a new process stub
    pub fn new(pid: Pid, ppid: Pid, name: String) -> Self {
        Self {
            pid,
            ppid,
            name,
            state: ProcessState::Ready,
            page_table_root: None,
            address_space: None,
        }
    }

    /// Get process ID
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Get parent process ID
    pub fn ppid(&self) -> Pid {
        self.ppid
    }

    /// Get process state
    pub fn state(&self) -> ProcessState {
        self.state
    }

    /// Set process state
    pub fn set_state(&mut self, state: ProcessState) {
        self.state = state;
    }

    /// Add a thread to this process (stub)
    pub fn add_thread(&mut self, _thread_id: u64) {
        // Stub for compatibility with loader
        // Full implementation in posix_x::core::process_state
    }
}

// ============================================================================
// GLOBAL PROCESS TABLE (Minimal stub)
// ============================================================================

use spin::Mutex;
use alloc::vec::Vec;
use alloc::sync::Arc;
use lazy_static::lazy_static;

lazy_static! {
    static ref PROCESS_TABLE: Mutex<Vec<Arc<Mutex<Process>>>> = Mutex::new(Vec::new());
    static ref NEXT_PID: spin::Mutex<Pid> = spin::Mutex::new(1);
}

/// Allocate a new unique PID
pub fn allocate_pid() -> Pid {
    let mut next = NEXT_PID.lock();
    let pid = *next;
    *next += 1;
    pid
}

/// Insert a process into the global table
pub fn insert_process(process: Arc<Mutex<Process>>) {
    let mut table = PROCESS_TABLE.lock();
    table.push(process);
}

/// Get a process by PID
pub fn get_process(pid: Pid) -> Option<Arc<Mutex<Process>>> {
    let table = PROCESS_TABLE.lock();
    table.iter()
        .find(|p| p.lock().pid == pid)
        .map(|p| Arc::clone(p))
}

/// Remove a process from the global table
pub fn remove_process(pid: Pid) -> Option<Arc<Mutex<Process>>> {
    let mut table = PROCESS_TABLE.lock();
    let index = table.iter().position(|p| p.lock().pid == pid)?;
    Some(table.remove(index))
}

/// Get all processes
pub fn all_processes() -> Vec<Arc<Mutex<Process>>> {
    let table = PROCESS_TABLE.lock();
    table.clone()
}

/// Initialize process subsystem
pub fn init() {
    log::debug!("Process subsystem initialized (minimal stub)");
}
