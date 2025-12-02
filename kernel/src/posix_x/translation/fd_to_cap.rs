//! FD to Capability Translation
//!
//! Converts POSIX file descriptors to Exo-OS capabilities

use crate::posix_x::core::fd_table::FdTable;
use alloc::string::String;
use alloc::vec::Vec;

/// Placeholder capability type
#[derive(Clone)]
pub struct Capability {
    pub cap_type: u8,
    pub rights: Vec<String>,
}

impl Capability {
    pub fn new(cap_type: u8) -> Self {
        Self {
            cap_type,
            rights: Vec::new(),
        }
    }

    pub fn add_right(&mut self, right: &str) {
        self.rights.push(String::from(right));
    }

    pub fn has_right(&self, right: &str) -> bool {
        self.rights.iter().any(|r| r == right)
    }
}

/// Convert FD to capability
pub fn fd_to_capability(fd: i32, fd_table: &FdTable) -> Option<Capability> {
    let handle = fd_table.get(fd)?;
    let handle_lock = handle.read();

    // Determine capability type based on handle type
    let cap_type = if handle_lock.path().starts_with("/dev/") {
        1 // Device
    } else if handle_lock.path().starts_with("/proc/") {
        2 // Process
    } else {
        0 // File
    };

    // Create capability with appropriate rights
    let mut cap = Capability::new(cap_type);

    // Set rights based on open flags
    if handle_lock.flags().read {
        cap.add_right("read");
    }
    if handle_lock.flags().write {
        cap.add_right("write");
    }

    Some(cap)
}

/// Convert FD to capability ID
pub fn fd_to_cap_id(fd: i32) -> u64 {
    // Simple mapping: FD offset by a base value
    const CAP_ID_BASE: u64 = 0x10000;
    CAP_ID_BASE + (fd as u64)
}

/// Validate FD has required capability
pub fn validate_fd_capability(fd: i32, fd_table: &FdTable, required_rights: &[&str]) -> bool {
    let Some(cap) = fd_to_capability(fd, fd_table) else {
        return false;
    };

    // Check all required rights
    required_rights.iter().all(|right| cap.has_right(right))
}

/// Get all capabilities for open FDs
pub fn get_all_fd_capabilities(fd_table: &FdTable) -> Vec<(i32, Capability)> {
    fd_table
        .list_fds()
        .iter()
        .filter_map(|&fd| fd_to_capability(fd, fd_table).map(|cap| (fd, cap)))
        .collect()
}
