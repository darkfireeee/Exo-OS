//! POSIX Compatibility Layer
//!
//! Integration between POSIX semantics and capability system

use super::capability::{Capability, Right, RightSet};
use super::object::{Object, ObjectId, ObjectType};

/// Convert POSIX mode to RightSet
///
/// mode: POSIX permission bits (rwxrwxrwx)
/// uid/gid: Subject credentials
/// file_uid/file_gid: Object owner
pub fn mode_to_rights(mode: u32, uid: u32, gid: u32, file_uid: u32, file_gid: u32) -> RightSet {
    let mut rights = RightSet::new();

    // Determine which permission bits to check (owner/group/other)
    let bits = if uid == file_uid {
        // Owner permissions (bits 6-8)
        (mode >> 6) & 0o7
    } else if gid == file_gid {
        // Group permissions (bits 3-5)
        (mode >> 3) & 0o7
    } else {
        // Other permissions (bits 0-2)
        mode & 0o7
    };

    // Convert rwx bits to Rights
    if bits & 0o4 != 0 {
        rights.add(Right::Read);
    }
    if bits & 0o2 != 0 {
        rights.add(Right::Write);
        rights.add(Right::Append);
    }
    if bits & 0o1 != 0 {
        rights.add(Right::Execute);
    }

    rights
}

/// Convert RightSet to POSIX mode bits
pub fn rights_to_mode(rights: &RightSet) -> u32 {
    let mut mode = 0;

    // Owner permissions
    if rights.has(Right::Read) {
        mode |= 0o400;
    }
    if rights.has(Right::Write) {
        mode |= 0o200;
    }
    if rights.has(Right::Execute) {
        mode |= 0o100;
    }

    // Duplicate for group and other (simplified)
    if rights.has(Right::Read) {
        mode |= 0o044;
    }
    if rights.has(Right::Write) {
        mode |= 0o022;
    }
    if rights.has(Right::Execute) {
        mode |= 0o011;
    }

    mode
}

/// Create capability from file descriptor
///
/// Integrates with POSIX-X FD table
pub fn fd_to_capability(
    fd: i32,
    fd_table: &crate::posix_x::core::fd_table::FdTable,
) -> Option<Capability> {
    let handle = fd_table.get(fd)?;
    let handle_lock = handle.read();

    // Create object ID from FD
    let object_id = ObjectId::from_fd(fd);

    // Get rights from open flags
    let mut rights = RightSet::new();

    if handle_lock.flags().read {
        rights.add(Right::Read);
    }

    if handle_lock.flags().write {
        rights.add(Right::Write);
        rights.add(Right::Append);
    }

    if handle_lock.flags().nonblock {
        // Add async rights if needed
    }

    Some(Capability::new(object_id, rights))
}

/// Check POSIX uid/gid permissions
///
/// Returns true if access should be granted based on POSIX rules
pub fn uid_gid_check(uid: u32, gid: u32, object: &Object, required_mode: u32) -> bool {
    // Root always has access
    if uid == 0 {
        return true;
    }

    // Check owner
    if uid == object.owner() {
        return true; // Owner has full access (simplified)
    }

    // Check group
    if gid == object.group() {
        return true; // Group members have access (simplified)
    }

    // Others would check the mode bits here
    // TODO: Implement full POSIX mode checking

    false
}

/// Convert POSIX open flags to Rights
pub fn open_flags_to_rights(flags: i32) -> RightSet {
    let mut rights = RightSet::new();

    const O_RDONLY: i32 = 0;
    const O_WRONLY: i32 = 1;
    const O_RDWR: i32 = 2;
    const O_APPEND: i32 = 0x400;
    const O_TRUNC: i32 = 0x200;

    let access_mode = flags & 0x3;

    match access_mode {
        O_RDONLY => {
            rights.add(Right::Read);
        }
        O_WRONLY => {
            rights.add(Right::Write);
        }
        O_RDWR => {
            rights.add(Right::Read);
            rights.add(Right::Write);
        }
        _ => {}
    }

    if flags & O_APPEND != 0 {
        rights.add(Right::Append);
    }

    if flags & O_TRUNC != 0 {
        rights.add(Right::Truncate);
    }

    rights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_conversion() {
        // rw-r--r-- (0644)
        let mode = 0o644;

        // Owner (uid match)
        let owner_rights = mode_to_rights(mode, 1000, 1000, 1000, 1000);
        assert!(owner_rights.has(Right::Read));
        assert!(owner_rights.has(Right::Write));
        assert!(!owner_rights.has(Right::Execute));

        // Group (gid match)
        let group_rights = mode_to_rights(mode, 1001, 1000, 1000, 1000);
        assert!(group_rights.has(Right::Read));
        assert!(!group_rights.has(Right::Write));

        // Other
        let other_rights = mode_to_rights(mode, 1001, 1001, 1000, 1000);
        assert!(other_rights.has(Right::Read));
        assert!(!other_rights.has(Right::Write));
    }

    #[test]
    fn test_rights_to_mode() {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);

        let mode = rights_to_mode(&rights);

        // Should have read/write for owner, group and other
        assert_eq!(mode & 0o600, 0o600); // Owner rw-
        assert_eq!(mode & 0o060, 0o060); // Group rw-
        assert_eq!(mode & 0o006, 0o006); // Other rw-
    }

    #[test]
    fn test_open_flags_conversion() {
        let rdonly = open_flags_to_rights(0); // O_RDONLY
        assert!(rdonly.has(Right::Read));
        assert!(!rdonly.has(Right::Write));

        let wronly = open_flags_to_rights(1); // O_WRONLY
        assert!(!wronly.has(Right::Read));
        assert!(wronly.has(Right::Write));

        let rdwr = open_flags_to_rights(2); // O_RDWR
        assert!(rdwr.has(Right::Read));
        assert!(rdwr.has(Right::Write));
    }
}
