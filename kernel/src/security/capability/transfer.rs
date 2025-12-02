//! Capability Transfer
//!
//! Safe capability transfer between processes

use super::{Capability, CapabilityId, CapabilityTable};
use alloc::sync::Arc;

/// Transfer result
#[derive(Debug)]
pub enum TransferResult {
    /// Successfully transferred
    Success(CapabilityId),
    /// Source capability not found
    SourceNotFound,
    /// Insufficient rights to transfer
    InsufficientRights,
    /// Transfer not allowed
    NotAllowed,
}

/// Transfer capability to another process
///
/// Creates a new capability in target table
pub fn transfer_capability(
    source_table: &CapabilityTable,
    target_table: &mut CapabilityTable,
    cap_id: CapabilityId,
) -> TransferResult {
    // Get source capability
    let cap = match source_table.get(cap_id) {
        Some(c) => c,
        None => return TransferResult::SourceNotFound,
    };

    // TODO: Check if capability can be transferred
    // - Check Grant right
    // - Check transfer policy

    // Clone capability to target
    let new_cap = Capability::new(cap.object_id, cap.rights);
    let new_id = target_table.allocate(new_cap);

    TransferResult::Success(new_id)
}

/// Grant capability (transfer with reduced rights)
pub fn grant_capability(
    source_table: &CapabilityTable,
    target_table: &mut CapabilityTable,
    cap_id: CapabilityId,
    rights: super::RightSet,
) -> TransferResult {
    let cap = match source_table.get(cap_id) {
        Some(c) => c,
        None => return TransferResult::SourceNotFound,
    };

    // Check if we have the rights to grant
    if !cap.rights.has_all(&rights) {
        return TransferResult::InsufficientRights;
    }

    // Create new capability with reduced rights
    let new_cap = Capability::new(cap.object_id, rights);
    let new_id = target_table.allocate(new_cap);

    TransferResult::Success(new_id)
}

/// Share capability (both processes get access)
pub fn share_capability(
    source_table: &CapabilityTable,
    target_table: &mut CapabilityTable,
    cap_id: CapabilityId,
) -> TransferResult {
    // Same as transfer but source keeps capability
    transfer_capability(source_table, target_table, cap_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::capability::{Right, RightSet};
    use crate::security::object::ObjectId;

    #[test]
    fn test_transfer() {
        let mut source_table = CapabilityTable::new();
        let mut target_table = CapabilityTable::new();

        let mut rights = RightSet::new();
        rights.add(Right::Read);
        rights.add(Right::Write);

        let cap = Capability::new(ObjectId(1), rights);
        let cap_id = source_table.allocate(cap);

        match transfer_capability(&source_table, &mut target_table, cap_id) {
            TransferResult::Success(new_id) => {
                assert!(target_table.contains(new_id));
            }
            _ => panic!("Transfer failed"),
        }
    }
}
