//! relation_type.rs — Types and metadata of ExoFS relations
//!
//! Applied rules:
//!  - ARITH-02: checked arithmetic on weights
//!  - ONDISK-03: no AtomicU64 in repr(C) structs

// ─────────────────────────────────────────────────────────────────────────────
// RelationKind
// ─────────────────────────────────────────────────────────────────────────────

/// Semantic nature of a relation between two blobs / ExoFS objects.
///
/// Each variant has a fixed on-disk u8 value for serialization.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationKind {
    /// `from` is the parent directory of `to`.
    Parent = 0x01,
    /// `from` is a child of `to`.
    Child = 0x02,
    /// `from` is a symbolic link to `to`.
    Symlink = 0x03,
    /// `from` is a hard link to `to`.
    HardLink = 0x04,
    /// Reference counting relation (P-Blob).
    Refcount = 0x05,
    /// `from` is a snapshot of `to`.
    Snapshot = 0x06,
    /// `to` is the base from which `from` was snapshotted.
    SnapshotBase = 0x07,
    /// `from` and `to` share chunks (deduplication).
    Dedup = 0x08,
    /// `from` is a Copy-on-Write clone of `to`.
    Clone = 0x09,
    /// Arbitrary cross-reference (extensible).
    CrossRef = 0x0A,
    /// Data dependency: `to` must exist for `from` to be readable.
    DataDep = 0x0B,
    /// Associated metadata: `from` enriches metadata of `to`.
    Metadata = 0x0C,
}

impl RelationKind {
    /// Converts an on-disk byte to `RelationKind`.
    ///
    /// Returns `None` if the value is unknown.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Parent),
            0x02 => Some(Self::Child),
            0x03 => Some(Self::Symlink),
            0x04 => Some(Self::HardLink),
            0x05 => Some(Self::Refcount),
            0x06 => Some(Self::Snapshot),
            0x07 => Some(Self::SnapshotBase),
            0x08 => Some(Self::Dedup),
            0x09 => Some(Self::Clone),
            0x0A => Some(Self::CrossRef),
            0x0B => Some(Self::DataDep),
            0x0C => Some(Self::Metadata),
            _ => None,
        }
    }

    /// On-disk value.
    #[inline]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// `true` if the relation is hierarchical (parent/child).
    #[inline]
    pub fn is_hierarchical(self) -> bool {
        matches!(self, Self::Parent | Self::Child)
    }

    /// `true` if the relation is a link (symlink or hard link).
    #[inline]
    pub fn is_link(self) -> bool {
        matches!(self, Self::Symlink | Self::HardLink)
    }

    /// `true` if the relation implies content sharing (dedup/clone/snapshot).
    #[inline]
    pub fn is_content_sharing(self) -> bool {
        matches!(
            self,
            Self::Dedup | Self::Clone | Self::Snapshot | Self::SnapshotBase
        )
    }

    /// `true` if the relation constitutes a strong dependency (deletion impossible
    /// without cleaning up the relation).
    pub fn is_strong_dependency(self) -> bool {
        matches!(
            self,
            Self::Parent | Self::Refcount | Self::SnapshotBase | Self::DataDep
        )
    }

    /// Human-readable label (for logs, debug).
    pub fn label(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Symlink => "symlink",
            Self::HardLink => "hardlink",
            Self::Refcount => "refcount",
            Self::Snapshot => "snapshot",
            Self::SnapshotBase => "snapshot_base",
            Self::Dedup => "dedup",
            Self::Clone => "clone",
            Self::CrossRef => "crossref",
            Self::DataDep => "data_dep",
            Self::Metadata => "metadata",
        }
    }

    /// `true` if the relation is directed (from→to has an asymmetric direction).
    pub fn is_directed(self) -> bool {
        !matches!(self, Self::Dedup | Self::CrossRef)
    }

    /// Returns the natural inverse kind if one exists.
    pub fn inverse(self) -> Option<Self> {
        match self {
            Self::Parent => Some(Self::Child),
            Self::Child => Some(Self::Parent),
            Self::Snapshot => Some(Self::SnapshotBase),
            Self::SnapshotBase => Some(Self::Snapshot),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationWeight
// ─────────────────────────────────────────────────────────────────────────────

/// Weight / priority of a relation for graph algorithms.
///
/// A weight is an unsigned integer; 0 is reserved (neutral relation).
/// The higher the weight, the "stronger" the relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationWeight(pub u32);

impl RelationWeight {
    /// Default weight (ordinary relations).
    pub const DEFAULT: Self = Self(1);
    /// Weight of a strong relation (e.g. counted reference).
    pub const STRONG: Self = Self(10);
    /// Maximum allowed weight.
    pub const MAX: Self = Self(u32::MAX);
    /// Zero weight (disabled relation / tombstone marker).
    pub const ZERO: Self = Self(0);

    /// Adds two weights with saturation.
    #[inline]
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Subtracts two weights with saturation (never negative).
    #[inline]
    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// `true` if this weight indicates a strong relation (≥ STRONG).
    #[inline]
    pub fn is_strong(self) -> bool {
        self.0 >= Self::STRONG.0
    }
}

impl Default for RelationWeight {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationFlags
// ─────────────────────────────────────────────────────────────────────────────

/// Additional boolean flags on a relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RelationFlags(pub u16);

impl RelationFlags {
    /// Relation marked as deleted (soft-delete).
    pub const DELETED: Self = Self(0x0001);
    /// Relation verified (validated integrity).
    pub const VERIFIED: Self = Self(0x0002);
    /// Automatically generated relation (not by user).
    pub const AUTO: Self = Self(0x0004);
    /// Temporary relation (can be purged without warning).
    pub const TEMPORARY: Self = Self(0x0008);

    pub fn has(self, flag: RelationFlags) -> bool {
        self.0 & flag.0 != 0
    }

    pub fn set(self, flag: RelationFlags) -> Self {
        Self(self.0 | flag.0)
    }

    pub fn clear(self, flag: RelationFlags) -> Self {
        Self(self.0 & !flag.0)
    }

    pub fn is_deleted(self) -> bool {
        self.has(Self::DELETED)
    }
    pub fn is_verified(self) -> bool {
        self.has(Self::VERIFIED)
    }
    pub fn is_temporary(self) -> bool {
        self.has(Self::TEMPORARY)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationType
// ─────────────────────────────────────────────────────────────────────────────

/// Complete relation type: kind + weight + flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationType {
    pub kind: RelationKind,
    pub weight: RelationWeight,
    pub flags: RelationFlags,
}

impl RelationType {
    /// Creates a type with default weight and without flags.
    pub const fn new(kind: RelationKind) -> Self {
        Self {
            kind,
            weight: RelationWeight::DEFAULT,
            flags: RelationFlags(0),
        }
    }

    /// Creates a type with explicit weight.
    pub const fn with_weight(kind: RelationKind, weight: u32) -> Self {
        Self {
            kind,
            weight: RelationWeight(weight),
            flags: RelationFlags(0),
        }
    }

    /// Adds a flag.
    pub fn with_flag(mut self, flag: RelationFlags) -> Self {
        self.flags = self.flags.set(flag);
        self
    }

    /// `true` if the relation is not deleted and has non-zero weight.
    pub fn is_active(self) -> bool {
        !self.flags.is_deleted() && self.weight != RelationWeight::ZERO
    }

    /// u32 weight for serialization.
    pub fn weight_u32(self) -> u32 {
        self.weight.0
    }

    /// u16 flags for serialization.
    pub fn flags_u16(self) -> u16 {
        self.flags.0
    }
}

impl Default for RelationType {
    fn default() -> Self {
        Self::new(RelationKind::CrossRef)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationDirection — query orientation
// ─────────────────────────────────────────────────────────────────────────────

/// Traversal direction of a relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationDirection {
    /// Outgoing relations (from → to).
    Outgoing,
    /// Incoming relations (to ← from).
    Incoming,
    /// Both directions.
    Both,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationFilter — selection filter for queries
// ─────────────────────────────────────────────────────────────────────────────

/// Filtering criteria for queries on relations.
#[derive(Clone, Copy, Debug, Default)]
pub struct RelationFilter {
    /// Restrict to a given kind (None = all).
    pub kind: Option<RelationKind>,
    /// Minimum weight (None = no minimum).
    pub min_weight: Option<u32>,
    /// Exclude soft-deleted relations.
    pub active_only: bool,
    /// Search direction.
    pub direction: Option<RelationDirection>,
}

impl RelationFilter {
    /// Filter accepting all active outgoing relations.
    pub const fn outgoing_active() -> Self {
        RelationFilter {
            kind: None,
            min_weight: None,
            active_only: true,
            direction: Some(RelationDirection::Outgoing),
        }
    }

    /// Filter for a specific kind, active only.
    pub fn by_kind(kind: RelationKind) -> Self {
        RelationFilter {
            kind: Some(kind),
            min_weight: None,
            active_only: true,
            direction: None,
        }
    }

    /// `true` if `RelationType` matches this filter.
    pub fn matches(self, rt: RelationType) -> bool {
        if self.active_only && !rt.is_active() {
            return false;
        }
        if let Some(k) = self.kind {
            if rt.kind != k {
                return false;
            }
        }
        if let Some(min_w) = self.min_weight {
            if rt.weight.0 < min_w {
                return false;
            }
        }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_roundtrip() {
        for v in 0x01u8..=0x0C {
            let k = RelationKind::from_u8(v).test_unwrap();
            assert_eq!(k.to_u8(), v);
        }
    }

    #[test]
    fn test_kind_unknown() {
        assert!(RelationKind::from_u8(0x00).is_none());
        assert!(RelationKind::from_u8(0xFF).is_none());
    }

    #[test]
    fn test_inverse() {
        assert_eq!(RelationKind::Parent.inverse(), Some(RelationKind::Child));
        assert_eq!(RelationKind::CrossRef.inverse(), None);
    }

    #[test]
    fn test_weight_saturating() {
        let w = RelationWeight(u32::MAX);
        assert_eq!(w.saturating_add(RelationWeight(1)), RelationWeight::MAX);
        assert_eq!(
            RelationWeight(0).saturating_sub(RelationWeight(5)),
            RelationWeight::ZERO
        );
    }

    #[test]
    fn test_flags_operations() {
        let f = RelationFlags::default();
        let f = f.set(RelationFlags::DELETED);
        assert!(f.is_deleted());
        let f = f.clear(RelationFlags::DELETED);
        assert!(!f.is_deleted());
    }

    #[test]
    fn test_type_active() {
        let t = RelationType::new(RelationKind::Parent);
        assert!(t.is_active());
        let t2 = t.with_flag(RelationFlags::DELETED);
        assert!(!t2.is_active());
    }

    #[test]
    fn test_filter_by_kind() {
        let f = RelationFilter::by_kind(RelationKind::Snapshot);
        let t_ok = RelationType::new(RelationKind::Snapshot);
        let t_ko = RelationType::new(RelationKind::Clone);
        assert!(f.matches(t_ok));
        assert!(!f.matches(t_ko));
    }

    #[test]
    fn test_is_hierarchical() {
        assert!(RelationKind::Parent.is_hierarchical());
        assert!(!RelationKind::Dedup.is_hierarchical());
    }

    #[test]
    fn test_label_completeness() {
        let kinds = [
            RelationKind::Parent,
            RelationKind::Child,
            RelationKind::Symlink,
            RelationKind::HardLink,
            RelationKind::Refcount,
            RelationKind::Snapshot,
            RelationKind::SnapshotBase,
            RelationKind::Dedup,
            RelationKind::Clone,
            RelationKind::CrossRef,
            RelationKind::DataDep,
            RelationKind::Metadata,
        ];
        for k in kinds {
            assert!(!k.label().is_empty());
        }
    }

    #[test]
    fn test_strong_dependency() {
        assert!(RelationKind::Refcount.is_strong_dependency());
        assert!(!RelationKind::Symlink.is_strong_dependency());
    }

    #[test]
    fn test_content_sharing() {
        assert!(RelationKind::Dedup.is_content_sharing());
        assert!(RelationKind::Clone.is_content_sharing());
        assert!(!RelationKind::Child.is_content_sharing());
    }

    #[test]
    fn test_directed() {
        assert!(RelationKind::Parent.is_directed());
        assert!(!RelationKind::Dedup.is_directed());
    }

    #[test]
    fn test_filter_active_only() {
        let f = RelationFilter {
            active_only: true,
            ..Default::default()
        };
        let deleted = RelationType::new(RelationKind::Clone).with_flag(RelationFlags::DELETED);
        assert!(!f.matches(deleted));
    }

    #[test]
    fn test_weight_is_strong() {
        assert!(RelationWeight::STRONG.is_strong());
        assert!(!RelationWeight::DEFAULT.is_strong());
    }
}
