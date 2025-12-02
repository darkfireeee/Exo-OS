//! Capability Rights Management
//!
//! Extended rights helpers and utilities

use super::{Right, RightSet};

/// Predefined common right sets
pub mod presets {
    use super::*;

    /// Read-only rights
    pub fn read_only() -> RightSet {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights
    }

    /// Read-write rights
    pub fn read_write() -> RightSet {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);
        rights.add(Right::Append);
        rights
    }

    /// Full file rights
    pub fn file_all() -> RightSet {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);
        rights.add(Right::Execute);
        rights.add(Right::Append);
        rights.add(Right::Truncate);
        rights.add(Right::Delete);
        rights
    }

    /// Directory rights
    pub fn directory_all() -> RightSet {
        let mut rights = RightSet::new();
        rights.add(Right::ListDir);
        rights.add(Right::CreateFile);
        rights.add(Right::DeleteFile);
        rights.add(Right::CreateDir);
        rights.add(Right::DeleteDir);
        rights.add(Right::Rename);
        rights
    }

    /// Memory rights
    pub fn memory_rw() -> RightSet {
        let mut rights = RightSet::new();
        rights.add(Right::MapRead);
        rights.add(Right::MapWrite);
        rights
    }

    /// Memory execute rights
    pub fn memory_rwx() -> RightSet {
        let mut rights = memory_rw();
        rights.add(Right::MapExecute);
        rights
    }
}

/// Check if rights are valid for object type
pub fn validate_rights_for_type(rights: &RightSet, _object_type: u8) -> bool {
    // TODO: Implement type-specific validation
    !rights.is_empty()
}

/// Get human-readable description of rights
pub fn describe_rights(rights: &RightSet) -> alloc::string::String {
    use alloc::string::String;
    use alloc::vec::Vec;

    let mut desc = Vec::new();

    if rights.has(Right::Read) {
        desc.push("read");
    }
    if rights.has(Right::Write) {
        desc.push("write");
    }
    if rights.has(Right::Execute) {
        desc.push("execute");
    }
    if rights.has(Right::Append) {
        desc.push("append");
    }

    if desc.is_empty() {
        String::from("no rights")
    } else {
        desc.join(", ")
    }
}
