//! Capability Tokens
//!
//! Serializable capability tokens for IPC

use super::{Capability, CapabilityId, RightSet};
use crate::security::object::ObjectId;

/// Capability token for transfer
///
/// Size:  32 bytes (compact for IPC)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct CapabilityToken {
    /// Capability ID
    pub id: u64,
    /// Object ID
    pub object_id: u64,
    /// Rights bitset
    pub rights: u64,
    /// Flags
    pub flags: u32,
    /// Reserved
    _reserved: u32,
}

impl CapabilityToken {
    /// Create token from capability
    pub fn from_capability(cap: &Capability) -> Self {
        Self {
            id: cap.id.0,
            object_id: cap.object_id.0,
            rights: cap.rights.to_bits(),
            flags: cap.metadata.flags,
            _reserved: 0,
        }
    }

    /// Convert to capability (unsafe - no validation)
    pub unsafe fn to_capability_unchecked(&self) -> Capability {
        Capability::with_id(
            CapabilityId(self.id),
            ObjectId(self.object_id),
            RightSet::from_bits(self.rights),
        )
    }

    /// Validate token structure
    pub fn is_valid(&self) -> bool {
        self.id != 0 && self.object_id != 0
    }
}

impl RightSet {
    /// Get raw bits (internal use only)
    pub(crate) fn to_bits(&self) -> u64 {
        // Access private field via unsafe (this is in same crate)
        unsafe { core::ptr::read(&self as *const Self as *const u64) }
    }

    /// Create from raw bits (internal use only)
    pub(crate) fn from_bits(bits: u64) -> Self {
        Self { bits }
    }
}

/// Serialize token to bytes
pub fn serialize_token(token: &CapabilityToken) -> [u8; 32] {
    unsafe { core::mem::transmute(*token) }
}

/// Deserialize token from bytes
pub fn deserialize_token(bytes: &[u8; 32]) -> CapabilityToken {
    unsafe { core::mem::transmute(*bytes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_serialization() {
        let mut rights = RightSet::new();
        rights.add(super::super::Right::Read);

        let cap = Capability::new(ObjectId(42), rights);
        let token = CapabilityToken::from_capability(&cap);

        assert!(token.is_valid());

        let bytes = serialize_token(&token);
        let token2 = deserialize_token(&bytes);

        assert_eq!(token.object_id, token2.object_id);
        assert_eq!(token.rights, token2.rights);
    }
}
