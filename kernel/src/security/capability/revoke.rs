//! Capability Revocation
//!
//! Safe capability revocation with dependency tracking

use super::{Capability, CapabilityId, CapabilityTable};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Revocation result
#[derive(Debug)]
pub enum RevocationResult {
    /// Successfully revoked
    Success,
    /// Capability not found
    NotFound,
    /// Capability has active derivations
    HasDerivations(Vec<CapabilityId>),
}

/// Revoke capability and all its derivations
///
/// Use with caution - cascading revocations!
pub fn revoke_cascade(table: &mut CapabilityTable, cap_id: CapabilityId) -> RevocationResult {
    // Find all derivations
    let derivations = find_derivations(table, cap_id);

    if !derivations.is_empty() {
        return RevocationResult::HasDerivations(derivations);
    }

    // Revoke the capability
    if table.revoke(cap_id) {
        RevocationResult::Success
    } else {
        RevocationResult::NotFound
    }
}

/// Find all capabilities derived from this one
pub fn find_derivations(table: &CapabilityTable, parent_id: CapabilityId) -> Vec<CapabilityId> {
    let mut derivations = Vec::new();

    for cap_id in table.list_ids() {
        if let Some(cap) = table.get(cap_id) {
            if cap.metadata.parent == Some(parent_id) {
                derivations.push(cap_id);
            }
        }
    }

    derivations
}

/// Revoke all capabilities for an object
pub fn revoke_all_for_object(
    table: &mut CapabilityTable,
    object_id: crate::security::object::ObjectId,
) -> usize {
    let to_revoke: Vec<_> = table
        .list_ids()
        .into_iter()
        .filter(|&cap_id| {
            table
                .get(cap_id)
                .map(|cap| cap.object_id == object_id)
                .unwrap_or(false)
        })
        .collect();

    let count = to_revoke.len();
    for cap_id in to_revoke {
        table.revoke(cap_id);
    }

    count
}

/// Check if capability can be safely revoked
pub fn can_revoke_safely(table: &CapabilityTable, cap_id: CapabilityId) -> bool {
    find_derivations(table, cap_id).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::capability::{Right, RightSet};
    use crate::security::object::ObjectId;

    #[test]
    fn test_revocation() {
        let mut table = CapabilityTable::new();

        let mut rights = RightSet::new();
        rights.add(Right::Read);

        let cap = Capability::new(ObjectId(1), rights);
        let cap_id = table.allocate(cap);

        assert!(can_revoke_safely(&table, cap_id));

        match revoke_cascade(&mut table, cap_id) {
            RevocationResult::Success => {}
            _ => panic!("Should succeed"),
        }

        assert!(!table.contains(cap_id));
    }
}
