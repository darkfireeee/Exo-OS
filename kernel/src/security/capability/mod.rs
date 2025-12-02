//! Capability System - Core Types
//!
//! High-performance capability implementation with:
//! - Compact representation (64 bytes)
//! - Cache-line alignment
//! - Lock-free access via Arc
//! - Bitset rights for O(1) checks

use super::object::ObjectId;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Capability ID - unique identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CapabilityId(pub u64);

impl CapabilityId {
    pub const INVALID: Self = Self(0);

    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn next(current: &AtomicU64) -> Self {
        Self(current.fetch_add(1, Ordering::Relaxed))
    }
}

/// Rights - fine-grained permissions
///
/// Stored as bitset for O(1) checks and minimal memory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Right {
    // File operations (0-7)
    Read = 0,
    Write = 1,
    Execute = 2,
    Append = 3,
    Truncate = 4,
    Delete = 5,
    Chmod = 6,
    Chown = 7,

    // Directory operations (8-15)
    ListDir = 8,
    CreateFile = 9,
    DeleteFile = 10,
    CreateDir = 11,
    DeleteDir = 12,
    Rename = 13,
    Link = 14,
    DeviceMmap = 43,
    DeviceIrq = 44,
    DeviceDma = 45,

    // Administrative (48-55)
    AdminRead = 48,
    AdminWrite = 49,
    AdminExecute = 50,
    SystemControl = 51,
    ModuleLoad = 52,
    Debug = 53,
    Audit = 54,
    Security = 55,
}

impl Right {
    pub fn as_bit(self) -> u64 {
        1u64 << (self as u8)
    }
}

/// RightSet - efficient set of rights
///
/// Uses bitset for O(1) operations and cache-friendly size (8 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct RightSet {
    bits: u64,
}

impl RightSet {
    /// Create empty rights
    #[inline]
    pub const fn new() -> Self {
        Self { bits: 0 }
    }

    /// Create with all rights
    #[inline]
    pub const fn all() -> Self {
        Self { bits: u64::MAX }
    }

    /// Create from single right
    #[inline]
    pub const fn from_right(right: Right) -> Self {
        Self {
            bits: 1u64 << (right as u8),
        }
    }

    /// Add a right - O(1)
    #[inline]
    pub fn add(&mut self, right: Right) {
        self.bits |= right.as_bit();
    }

    /// Remove a right - O(1)
    #[inline]
    pub fn remove(&mut self, right: Right) {
        self.bits &= !right.as_bit();
    }

    /// Check if has right - O(1)
    #[inline]
    pub fn has(&self, right: Right) -> bool {
        (self.bits & right.as_bit()) != 0
    }

    /// Check if has all rights - O(1)
    #[inline]
    pub fn has_all(&self, other: &RightSet) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Union of two sets - O(1)
    #[inline]
    pub fn union(&self, other: &RightSet) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// Intersection - O(1)
    #[inline]
    pub fn intersection(&self, other: &RightSet) -> Self {
        Self {
            bits: self.bits & other.bits,
        }
    }

    /// Is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Count rights - O(1) with popcount
    #[inline]
    pub fn count(&self) -> u32 {
        self.bits.count_ones()
    }

    /// From POSIX mode bits
    pub fn from_posix_mode(mode: u32) -> Self {
        let mut rights = Self::new();

        // Read permission
        if mode & 0o444 != 0 {
            rights.add(Right::Read);
        }

        // Write permission
        if mode & 0o222 != 0 {
            rights.add(Right::Write);
            rights.add(Right::Append);
        }

        // Execute permission
        if mode & 0o111 != 0 {
            rights.add(Right::Execute);
        }

        rights
    }

    /// To POSIX mode bits
    pub fn to_posix_mode(&self) -> u32 {
        let mut mode = 0;

        if self.has(Right::Read) {
            mode |= 0o444;
        }
        if self.has(Right::Write) {
            mode |= 0o222;
        }
        if self.has(Right::Execute) {
            mode |= 0o111;
        }

        mode
    }
}

/// Capability metadata
#[derive(Debug, Clone)]
pub struct CapabilityMetadata {
    /// Original parent capability (for derivation chain)
    pub parent: Option<CapabilityId>,
    /// Creation timestamp (nanoseconds)
    pub created_at: u64,
    /// Access counter
    pub access_count: u64,
    /// Flags
    pub flags: u32,
}

impl CapabilityMetadata {
    pub fn new() -> Self {
        Self {
            parent: None,
            created_at: 0, // TODO: Real timestamp
            access_count: 0,
            flags: 0,
        }
    }
}

/// Capability - grants access to an object with specific rights
///
/// Compact size: 64 bytes (cache-line friendly)
#[derive(Debug, Clone)]
#[repr(align(64))] // Cache-line alignment for performance
pub struct Capability {
    /// Unique capability ID
    pub id: CapabilityId,
    /// Object this capability grants access to
    pub object_id: ObjectId,
    /// Rights granted by this capability
    pub rights: RightSet,
    /// Metadata
    pub metadata: CapabilityMetadata,
}

impl Capability {
    /// Create new capability
    pub fn new(object_id: ObjectId, rights: RightSet) -> Self {
        Self {
            id: CapabilityId::INVALID, // Will be set by table
            object_id,
            rights,
            metadata: CapabilityMetadata::new(),
        }
    }

    /// Create with specific ID
    pub fn with_id(id: CapabilityId, object_id: ObjectId, rights: RightSet) -> Self {
        Self {
            id,
            object_id,
            rights,
            metadata: CapabilityMetadata::new(),
        }
    }

    /// Derive new capability with subset of rights
    pub fn derive(&self, rights: RightSet) -> Result<Self, &'static str> {
        // Can only derive subset of rights
        if !self.rights.has_all(&rights) {
            return Err("Cannot derive capability with more rights than parent");
        }

        let mut derived = Self::new(self.object_id, rights);
        derived.metadata.parent = Some(self.id);
        Ok(derived)
    }

    /// Check if has specific right - O(1)
    #[inline]
    pub fn has_right(&self, right: Right) -> bool {
        self.rights.has(right)
    }

    /// Check if has all required rights - O(1)
    #[inline]
    pub fn has_rights(&self, required: &RightSet) -> bool {
        self.rights.has_all(required)
    }
}

/// Per-process capability table
///
/// Fast lookups: O(1) via BTreeMap
/// Lock-free reads via Arc
pub struct CapabilityTable {
    /// Capabilities indexed by ID
    capabilities: BTreeMap<CapabilityId, Arc<Capability>>,
    /// Next capability ID
    next_id: AtomicU64,
}

impl CapabilityTable {
    /// Create new empty table
    pub fn new() -> Self {
        Self {
            capabilities: BTreeMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Allocate new capability - O(log n)
    pub fn allocate(&mut self, mut cap: Capability) -> CapabilityId {
        let cap_id = CapabilityId::next(&self.next_id);
        cap.id = cap_id;
        self.capabilities.insert(cap_id, Arc::new(cap));
        cap_id
    }

    /// Get capability by ID - O(log n), returns Arc for zero-copy
    #[inline]
    pub fn get(&self, cap_id: CapabilityId) -> Option<Arc<Capability>> {
        self.capabilities.get(&cap_id).cloned()
    }

    /// Check if capability exists - O(log n)
    #[inline]
    pub fn contains(&self, cap_id: CapabilityId) -> bool {
        self.capabilities.contains_key(&cap_id)
    }

    /// Revoke capability - O(log n)
    pub fn revoke(&mut self, cap_id: CapabilityId) -> bool {
        self.capabilities.remove(&cap_id).is_some()
    }

    /// Derive new capability from existing - O(log n)
    pub fn derive(
        &mut self,
        parent_id: CapabilityId,
        rights: RightSet,
    ) -> Result<CapabilityId, &'static str> {
        let parent = self.get(parent_id).ok_or("Parent capability not found")?;

        let derived = parent.derive(rights)?;
        Ok(self.allocate(derived))
    }

    /// Count capabilities
    #[inline]
    pub fn count(&self) -> usize {
        self.capabilities.len()
    }

    /// Clone table for fork() - uses Arc so no actual copy
    pub fn clone_table(&self) -> Self {
        Self {
            capabilities: self.capabilities.clone(),
            next_id: AtomicU64::new(self.next_id.load(Ordering::Relaxed)),
        }
    }

    /// List all capability IDs
    pub fn list_ids(&self) -> Vec<CapabilityId> {
        self.capabilities.keys().copied().collect()
    }
}

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::object::ObjectId;

    #[test]
    fn test_right_set_operations() {
        let mut rights = RightSet::new();
        assert!(rights.is_empty());

        rights.add(Right::Read);
        assert!(rights.has(Right::Read));
        assert!(!rights.has(Right::Write));

        rights.add(Right::Write);
        assert!(rights.has(Right::Write));
        assert_eq!(rights.count(), 2);

        rights.remove(Right::Read);
        assert!(!rights.has(Right::Read));
        assert_eq!(rights.count(), 1);
    }

    #[test]
    fn test_right_set_union_intersection() {
        let mut r1 = RightSet::new();
        r1.add(Right::Read);
        r1.add(Right::Write);

        let mut r2 = RightSet::new();
        r2.add(Right::Write);
        r2.add(Right::Execute);

        let union = r1.union(&r2);
        assert!(union.has(Right::Read));
        assert!(union.has(Right::Write));
        assert!(union.has(Right::Execute));

        let intersection = r1.intersection(&r2);
        assert!(!intersection.has(Right::Read));
        assert!(intersection.has(Right::Write));
        assert!(!intersection.has(Right::Execute));
    }

    #[test]
    fn test_capability_derivation() {
        let mut table = CapabilityTable::new();

        let mut full_rights = RightSet::new();
        full_rights.add(Right::Read);
        full_rights.add(Right::Write);

        let parent_cap = Capability::new(ObjectId(1), full_rights);
        let parent_id = table.allocate(parent_cap);

        // Derive with only read
        let mut read_only = RightSet::new();
        read_only.add(Right::Read);

        let child_id = table.derive(parent_id, read_only).unwrap();
        let child_cap = table.get(child_id).unwrap();

        assert!(child_cap.has_right(Right::Read));
        assert!(!child_cap.has_right(Right::Write));
    }

    #[test]
    fn test_posix_mode_conversion() {
        let mode = 0o644; // rw-r--r--
        let rights = RightSet::from_posix_mode(mode);

        assert!(rights.has(Right::Read));
        assert!(rights.has(Right::Write));
        assert!(!rights.has(Right::Execute));

        let mode_back = rights.to_posix_mode();
        assert_eq!(mode, mode_back);
    }
}
