//! Process Table
//!
//! Global table of all processes in the system.

use super::{Process, Pid};
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;

/// Global process table
pub struct ProcessTable {
    /// Map of PID -> Process
    processes: BTreeMap<Pid, Arc<Mutex<Process>>>,
}

impl ProcessTable {
    /// Create new empty process table
    pub const fn new() -> Self {
        Self {
            processes: BTreeMap::new(),
        }
    }
    
    /// Insert process into table
    pub fn insert(&mut self, pid: Pid, process: Arc<Mutex<Process>>) {
        self.processes.insert(pid, process);
    }
    
    /// Get process by PID
    pub fn get(&self, pid: Pid) -> Option<Arc<Mutex<Process>>> {
        self.processes.get(&pid).cloned()
    }
    
    /// Remove process from table
    pub fn remove(&mut self, pid: Pid) -> Option<Arc<Mutex<Process>>> {
        self.processes.remove(&pid)
    }
    
    /// Get number of processes
    pub fn len(&self) -> usize {
        self.processes.len()
    }
    
    /// Check if table is empty
    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }
    
    /// Iterate over all processes
    pub fn iter(&self) -> impl Iterator<Item = (&Pid, &Arc<Mutex<Process>>)> {
        self.processes.iter()
    }
}

/// Global process table instance
static PROCESS_TABLE: Mutex<ProcessTable> = Mutex::new(ProcessTable::new());

/// Get reference to global process table
pub fn process_table() -> &'static Mutex<ProcessTable> {
    &PROCESS_TABLE
}

/// Get process by PID
pub fn get_process(pid: Pid) -> Option<Arc<Mutex<Process>>> {
    PROCESS_TABLE.lock().get(pid)
}

/// Insert process into global table
pub fn insert_process(pid: Pid, process: Arc<Mutex<Process>>) {
    PROCESS_TABLE.lock().insert(pid, process);
}

/// Remove process from global table
pub fn remove_process(pid: Pid) -> Option<Arc<Mutex<Process>>> {
    PROCESS_TABLE.lock().remove(pid)
}

/// Get all zombie processes
pub fn get_zombies() -> alloc::vec::Vec<Pid> {
    use super::ProcessState;
    
    let table = PROCESS_TABLE.lock();
    let mut zombies = alloc::vec::Vec::new();
    
    for (pid, process_arc) in table.iter() {
        let process = process_arc.lock();
        if process.state == ProcessState::Zombie {
            zombies.push(*pid);
        }
    }
    
    zombies
}

/// Get children of a process
pub fn get_children(parent_pid: Pid) -> alloc::vec::Vec<Pid> {
    let table = PROCESS_TABLE.lock();
    let mut children = alloc::vec::Vec::new();
    
    for (pid, process_arc) in table.iter() {
        let process = process_arc.lock();
        if process.parent_pid == Some(parent_pid) {
            children.push(*pid);
        }
    }
    
    children
}

/// Print process table (for debugging)
pub fn print_process_table() {
    let table = PROCESS_TABLE.lock();
    
    crate::logger::info("=== Process Table ===");
    let s = alloc::format!("Total processes: {}", table.len());
    crate::logger::info(&s);
    
    for (pid, process_arc) in table.iter() {
        let process = process_arc.lock();
        let s = alloc::format!(
            "PID {} ({}) - Parent: {:?}, Threads: {}, State: {:?}",
            pid,
            process.name,
            process.parent_pid,
            process.threads.len(),
            process.state
        );
        crate::logger::info(&s);
    }
}
