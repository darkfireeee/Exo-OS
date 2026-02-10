//! Permission Checking - POSIX rwx permissions
//!
//! Provides permission checking functions for filesystem operations.

use crate::fs::{FsError, FsResult};
use crate::fs::core::InodePermissions;

/// Check if user has read permission
pub fn check_read(uid: u32, gid: u32, file_uid: u32, file_gid: u32, mode: InodePermissions) -> FsResult<()> {
    if mode.can_read(uid, gid, file_uid, file_gid) {
        Ok(())
    } else {
        Err(FsError::PermissionDenied)
    }
}

/// Check if user has write permission
pub fn check_write(uid: u32, gid: u32, file_uid: u32, file_gid: u32, mode: InodePermissions) -> FsResult<()> {
    if mode.can_write(uid, gid, file_uid, file_gid) {
        Ok(())
    } else {
        Err(FsError::PermissionDenied)
    }
}

/// Check if user has execute permission
pub fn check_execute(uid: u32, gid: u32, file_uid: u32, file_gid: u32, mode: InodePermissions) -> FsResult<()> {
    if mode.can_execute(uid, gid, file_uid, file_gid) {
        Ok(())
    } else {
        Err(FsError::PermissionDenied)
    }
}

/// Initialize permissions subsystem
pub fn init() {
    log::debug!("Permissions subsystem initialized");
}
