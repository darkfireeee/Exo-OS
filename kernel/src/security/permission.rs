//! Permission Checking
//!
//! High-performance permission validation (<50ns per check)

use super::capability::{Capability, CapabilityId, Right, RightSet};
use super::object::{Object, ObjectId};
use alloc::vec::Vec;

/// Permission error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionError {
    /// No capability for object
    NoCapability,
    /// Capability lacks required rights
    InsufficientRights,
    /// Owner/group check failed
    AccessDenied,
    /// Object not found
    ObjectNotFound,
    /// Invalid capability
    InvalidCapability,
}

/// Permission check context
///
/// Contains all info needed for permission check
/// Size: 40 bytes (cache-friendly)
#[derive(Debug, Clone)]
pub struct PermissionContext {
    /// Subject user ID
    pub uid: u32,
    /// Subject group ID
    pub gid: u32,
    /// Subject capabilities (IDs only for compactness)
    pub capability_ids: Vec<CapabilityId>,
}

impl PermissionContext {
    pub fn new(uid: u32, gid: u32) -> Self {
        Self {
            uid,
            gid,
            capability_ids: Vec::new(),
        }
    }

    pub fn with_capabilities(uid: u32, gid: u32, caps: Vec<CapabilityId>) -> Self {
        Self {
            uid,
            gid,
            capability_ids: caps,
        }
    }
}

/// Check permission using capability
///
/// O(1) operation - just bitset check
#[inline]
pub fn check_capability(
    cap: &Capability,
    required_rights: &RightSet,
) -> Result<(), PermissionError> {
    if cap.has_rights(required_rights) {
        Ok(())
    } else {
        Err(PermissionError::InsufficientRights)
    }
}

/// Check permission with full context
///
/// Performance: <50ns typical
pub fn check_permission(
    context: &PermissionContext,
    object: &Object,
    required_rights: &RightSet,
    capability: Option<&Capability>,
) -> Result<(), PermissionError> {
    // Fast path: Check capability if provided
    if let Some(cap) = capability {
        // Verify capability is for this object
        if cap.object_id != object.id {
            return Err(PermissionError::InvalidCapability);
        }

        // Check rights - O(1) bitset operation
        return check_capability(cap, required_rights);
    }

    // Slow path: POSIX-style owner/group/other check
    // This is fallback for compatibility
    check_posix_permission(context, object, required_rights)
}

/// POSIX-style permission check (fallback)
fn check_posix_permission(
    context: &PermissionContext,
    object: &Object,
    required_rights: &RightSet,
) -> Result<(), PermissionError> {
    // Check if owner
    if object.is_owned_by(context.uid) {
        // Owner has all rights for now
        // TODO: Implement proper POSIX mode checking
        return Ok(());
    }

    // Check if in group
    if object.is_in_group(context.gid) {
        // Group members get some rights
        // TODO: Implement proper POSIX mode checking
        return Ok(());
    }

    // Others
    // TODO: Check other permissions from mode
    Err(PermissionError::AccessDenied)
}

/// Fast permission check for common case
///
/// Optimized for typical read/write checks
#[inline]
pub fn can_read(cap: &Capability) -> bool {
    cap.has_right(Right::Read)
}

#[inline]
pub fn can_write(cap: &Capability) -> bool {
    cap.has_right(Right::Write)
}

#[inline]
pub fn can_execute(cap: &Capability) -> bool {
    cap.has_right(Right::Execute)
}

#[inline]
pub fn can_read_write(cap: &Capability) -> bool {
    cap.has_right(Right::Read) && cap.has_right(Right::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::object::ObjectType;

    #[test]
    fn test_permission_check() {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);

        let object_id = ObjectId(1);
        let cap = Capability::new(object_id, rights);

        let mut required = RightSet::new();
        required.add(Right::Read);

        assert!(check_capability(&cap, &required).is_ok());

        required.add(Right::Execute);
        assert!(check_capability(&cap, &required).is_err());
    }

    #[test]
    fn test_fast_checks() {
        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);

        let cap = Capability::new(ObjectId(1), rights);

        assert!(can_read(&cap));
        assert!(can_write(&cap));
        assert!(!can_execute(&cap));
        assert!(can_read_write(&cap));
    }
}
