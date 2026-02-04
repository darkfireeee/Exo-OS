//! Process Management
//!
//! Gestion des processus avec UserAddressSpace pour CoW.
//! Chaque Process a son propre espace d'adressage virtuel.

pub mod table;

pub use table::{insert_process, get_process, remove_process};

use crate::memory::{UserAddressSpace, MemoryError};
use crate::scheduler::ThreadId;
// use crate::fs::FdTable; // TODO: Implement FdTable
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};

// TODO: Proper FdTable implementation
pub type FdTable = ();

/// Process ID type
pub type Pid = u32;

/// Allocate next PID
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

pub fn allocate_pid() -> Pid {
    NEXT_PID.fetch_add(1, Ordering::SeqCst)
}

/// User credentials
#[derive(Debug, Clone, Copy)]
pub struct Credentials {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
}

impl Credentials {
    pub fn root() -> Self {
        Self {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
        }
    }
    
    pub fn new(uid: u32, gid: u32) -> Self {
        Self {
            uid,
            gid,
            euid: uid,
            egid: gid,
        }
    }
}

/// Process state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Running or runnable
    Running,
    /// Waiting for event
    Sleeping,
    /// Zombie (exited but not reaped)
    Zombie,
    /// Stopped (SIGSTOP)
    Stopped,
}

/// A process with its own address space
#[derive(Debug)]
pub struct Process {
    /// Process ID
    pub pid: Pid,
    
    /// Parent process ID
    pub parent_pid: Option<Pid>,
    
    /// Process name
    pub name: String,
    
    /// User address space (for CoW fork)
    pub address_space: UserAddressSpace,
    
    /// Main thread ID
    pub main_thread: ThreadId,
    
    /// All threads in this process
    pub threads: Vec<ThreadId>,
    
    /// File descriptor table
    pub fd_table: FdTable,
    
    /// User credentials
    pub credentials: Credentials,
    
    /// Process state
    pub state: ProcessState,
    
    /// Exit code (for zombies)
    pub exit_code: Option<i32>,
}

impl Process {
    /// Create new process
    pub fn new(
        pid: Pid,
        parent_pid: Option<Pid>,
        name: String,
        address_space: UserAddressSpace,
    ) -> Self {
        Self {
            pid,
            parent_pid,
            name,
            address_space,
            main_thread: 0,
            threads: Vec::new(),
            fd_table: (), // TODO: FdTable::new()
            credentials: Credentials::root(),
            state: ProcessState::Running,
            exit_code: None,
        }
    }
    
    /// Add thread to this process
    pub fn add_thread(&mut self, tid: ThreadId) {
        if self.threads.is_empty() {
            self.main_thread = tid;
        }
        self.threads.push(tid);
    }
    
    /// Remove thread from this process
    pub fn remove_thread(&mut self, tid: ThreadId) {
        self.threads.retain(|&t| t != tid);
    }
    
    /// Check if process has no more threads
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
    
    /// Mark process as zombie
    pub fn exit(&mut self, code: i32) {
        self.state = ProcessState::Zombie;
        self.exit_code = Some(code);
    }
}

/// Get current process from scheduler
pub fn get_current_process() -> Option<alloc::sync::Arc<crate::sync::Mutex<Process>>> {
    use crate::scheduler::SCHEDULER;
    
    SCHEDULER.with_current_thread(|thread| {
        thread.process()
    }).flatten()
}
