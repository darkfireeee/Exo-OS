//! relation.rs — Relation and ExoFS identifier
//!
//! Applied rules:
//!  - ONDISK-03: no AtomicU64 in repr(C, packed) structs
//!  - HDR-03   : magic verified first during on-disk parse
//!  - ARITH-02 : checked arithmetic on all offsets

use super::relation_type::{RelationFlags, RelationKind, RelationType, RelationWeight};
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Magic identifying a valid on-disk relation (b"RLTN").
pub const RELATION_MAGIC: u32 = 0x524C544E;

/// Fixed size of a relation on-disk block.
pub const RELATION_ONDISK_SIZE: usize = 96;

// ─────────────────────────────────────────────────────────────────────────────
// RelationId
// ─────────────────────────────────────────────────────────────────────────────

/// Unique relation identifier (64 bits, 0 = invalid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationId(pub u64);

impl RelationId {
    /// Invalid identifier (sentinel).
    pub const INVALID: Self = Self(0);

    /// `true` if this ID is valid.
    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }

    /// Generates next ID (wrapping, skips 0).
    pub fn next(self) -> Self {
        let n = self.0.wrapping_add(1);
        if n == 0 {
            Self(1)
        } else {
            Self(n)
        }
    }
}

impl Default for RelationId {
    fn default() -> Self {
        Self::INVALID
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationOnDisk — fixed binary structure (96 bytes)
// ─────────────────────────────────────────────────────────────────────────────

/// On-disk representation of a relation.
///
/// Fixed size 96 bytes for clean alignment.
/// No `AtomicU64` (ONDISK-03).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct RelationOnDisk {
    /// Magic RELATION_MAGIC (HDR-03: verified first).
    pub magic: u32,
    /// Kind u8.
    pub kind: u8,
    /// Flags u16.
    pub flags: u16,
    /// Padding for alignment.
    pub _pad0: u8,
    /// Relation identifier.
    pub id: u64,
    /// Source BlobId (32 bytes).
    pub from_blob: [u8; 32],
    /// Destination BlobId (32 bytes).
    pub to_blob: [u8; 32],
    /// Weight u32.
    pub weight: u32,
    /// Creation timestamp (CPU ticks).
    pub created_at: u64,
    /// Simple CRC32 of previous fields (validation).
    pub crc32: u32,
}

const _CHECK_ONDISK: () = assert!(core::mem::size_of::<RelationOnDisk>() == RELATION_ONDISK_SIZE);

impl RelationOnDisk {
    /// Parses from a byte slice (HDR-03: magic first, CRC validation).
    pub fn from_bytes(buf: &[u8; RELATION_ONDISK_SIZE]) -> ExofsResult<Self> {
        // HDR-03: read and validate magic first.
        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != RELATION_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        if !Self::crc_ok(buf) {
            return Err(ExofsError::ChecksumMismatch);
        }
        // Manual copy to prevent UB on packed.
        let mut d: [u8; RELATION_ONDISK_SIZE] = [0u8; RELATION_ONDISK_SIZE];
        d.copy_from_slice(buf);
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        Ok(unsafe { core::ptr::read_unaligned(d.as_ptr() as *const RelationOnDisk) })
    }

    /// Serializes into a 96-byte array.
    pub fn to_bytes(self) -> [u8; RELATION_ONDISK_SIZE] {
        let mut out = [0u8; RELATION_ONDISK_SIZE];
        // SAFETY: safety invariant verified by preconditions of calling function.
        unsafe {
            core::ptr::write_unaligned(out.as_mut_ptr() as *mut RelationOnDisk, self);
        }
        out
    }

    /// Computes a simplified CRC32 (FNV-1a over first 88 bytes).
    pub fn compute_crc(buf: &[u8; RELATION_ONDISK_SIZE]) -> u32 {
        let mut h: u32 = 0x811c9dc5u32;
        for &b in &buf[..RELATION_ONDISK_SIZE - 4] {
            h = h.wrapping_mul(0x01000193).wrapping_add(b as u32);
        }
        h
    }

    /// `true` if CRC is valid.
    pub fn crc_ok(buf: &[u8; RELATION_ONDISK_SIZE]) -> bool {
        let computed = Self::compute_crc(buf);
        let stored = u32::from_le_bytes([
            buf[RELATION_ONDISK_SIZE - 4],
            buf[RELATION_ONDISK_SIZE - 3],
            buf[RELATION_ONDISK_SIZE - 2],
            buf[RELATION_ONDISK_SIZE - 1],
        ]);
        computed == stored
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Relation — in-memory representation
// ─────────────────────────────────────────────────────────────────────────────

/// In-memory relation between two blobs.
#[derive(Clone, Debug)]
pub struct Relation {
    /// Unique identifier.
    pub id: RelationId,
    /// Source blob.
    pub from: BlobId,
    /// Destination blob.
    pub to: BlobId,
    /// Complete relation type.
    pub rel_type: RelationType,
    /// CPU ticks at creation.
    pub created_at: u64,
    /// CPU ticks at last modification.
    pub updated_at: u64,
}

impl Relation {
    /// Full constructor.
    pub fn new(
        id: RelationId,
        from: BlobId,
        to: BlobId,
        rel_type: RelationType,
        created_at: u64,
    ) -> Self {
        Relation {
            id,
            from,
            to,
            rel_type,
            updated_at: created_at,
            created_at,
        }
    }

    /// `true` if relation is active (not deleted, non-zero weight).
    #[inline]
    pub fn is_active(&self) -> bool {
        self.rel_type.is_active()
    }

    /// `true` if `from` == `to` (self-relation / simple loop).
    #[inline]
    pub fn is_self_loop(&self) -> bool {
        self.from.as_bytes() == self.to.as_bytes()
    }

    /// Serializes to `RelationOnDisk`.
    pub fn to_on_disk(&self) -> RelationOnDisk {
        let mut d = RelationOnDisk {
            magic: RELATION_MAGIC,
            kind: self.rel_type.kind.to_u8(),
            flags: self.rel_type.flags_u16(),
            _pad0: 0,
            id: self.id.0,
            from_blob: *self.from.as_bytes(),
            to_blob: *self.to.as_bytes(),
            weight: self.rel_type.weight_u32(),
            created_at: self.created_at,
            crc32: 0,
        };
        // Compute and insert CRC.
        let mut buf = d.to_bytes();
        let crc = RelationOnDisk::compute_crc(&buf);
        let crc_bytes = crc.to_le_bytes();
        buf[RELATION_ONDISK_SIZE - 4..].copy_from_slice(&crc_bytes);
        d.crc32 = crc;
        d
    }

    /// Deserializes from `RelationOnDisk`.
    pub fn from_on_disk(d: &RelationOnDisk) -> ExofsResult<Self> {
        let kind = RelationKind::from_u8(d.kind).ok_or(ExofsError::CorruptedStructure)?;
        let id = RelationId(d.id);
        if !id.is_valid() {
            return Err(ExofsError::CorruptedStructure);
        }
        let rel_type = RelationType {
            kind,
            weight: RelationWeight(d.weight),
            flags: RelationFlags(d.flags),
        };
        Ok(Relation {
            id,
            from: BlobId(d.from_blob),
            to: BlobId(d.to_blob),
            rel_type,
            created_at: d.created_at,
            updated_at: d.created_at,
        })
    }

    /// Marks relation as deleted (soft-delete).
    pub fn mark_deleted(&mut self, tick: u64) {
        self.rel_type.flags = self.rel_type.flags.set(RelationFlags::DELETED);
        self.updated_at = tick;
    }

    /// Returns a short summary (for logs).
    pub fn summary(&self) -> RelationSummary {
        RelationSummary {
            id: self.id,
            kind: self.rel_type.kind,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationSummary — lightweight version for callbacks
// ─────────────────────────────────────────────────────────────────────────────

/// Lightweight version of a relation (to avoid expensive clones).
#[derive(Clone, Copy, Debug)]
pub struct RelationSummary {
    pub id: RelationId,
    pub kind: RelationKind,
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationBuilder — fluent builder
// ─────────────────────────────────────────────────────────────────────────────

/// Builder to create a `Relation` explicitly.
pub struct RelationBuilder {
    id: RelationId,
    from: Option<BlobId>,
    to: Option<BlobId>,
    rel_type: RelationType,
    tick: u64,
}

impl RelationBuilder {
    pub fn new(id: RelationId, tick: u64) -> Self {
        RelationBuilder {
            id,
            from: None,
            to: None,
            rel_type: RelationType::default(),
            tick,
        }
    }

    pub fn from(mut self, b: BlobId) -> Self {
        self.from = Some(b);
        self
    }
    pub fn to(mut self, b: BlobId) -> Self {
        self.to = Some(b);
        self
    }
    pub fn kind(mut self, k: RelationKind) -> Self {
        self.rel_type.kind = k;
        self
    }
    pub fn weight(mut self, w: u32) -> Self {
        self.rel_type.weight = RelationWeight(w);
        self
    }

    /// Builds the relation.
    ///
    /// Returns `Err(InvalidArgument)` if `from` or `to` is missing.
    pub fn build(self) -> ExofsResult<Relation> {
        let from = self.from.ok_or(ExofsError::InvalidArgument)?;
        let to = self.to.ok_or(ExofsError::InvalidArgument)?;
        Ok(Relation::new(self.id, from, to, self.rel_type, self.tick))
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

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    #[test]
    fn test_relation_id_next_wraps() {
        let id = RelationId(u64::MAX);
        assert_eq!(id.next(), RelationId(1));
    }

    #[test]
    fn test_relation_roundtrip() {
        let rel = Relation::new(
            RelationId(42),
            blob(0xAA),
            blob(0xBB),
            RelationType::new(RelationKind::Parent),
            12345,
        );
        let on_disk = rel.to_on_disk();
        let back = Relation::from_on_disk(&on_disk).test_unwrap();
        assert_eq!(back.id, RelationId(42));
        assert_eq!(back.rel_type.kind, RelationKind::Parent);
        assert_eq!(back.from.as_bytes(), &[0xAA; 32]);
    }

    #[test]
    fn test_ondisk_parse_bad_magic() {
        let mut buf = [0u8; RELATION_ONDISK_SIZE];
        buf[0] = 0xDE;
        buf[1] = 0xAD;
        buf[2] = 0xBE;
        buf[3] = 0xEF;
        assert!(RelationOnDisk::from_bytes(&buf).is_err());
    }

    #[test]
    fn test_self_loop_detection() {
        let rel = Relation::new(
            RelationId(1),
            blob(5),
            blob(5),
            RelationType::new(RelationKind::CrossRef),
            0,
        );
        assert!(rel.is_self_loop());
    }

    #[test]
    fn test_mark_deleted() {
        let mut rel = Relation::new(
            RelationId(7),
            blob(1),
            blob(2),
            RelationType::new(RelationKind::Clone),
            100,
        );
        assert!(rel.is_active());
        rel.mark_deleted(200);
        assert!(!rel.is_active());
    }

    #[test]
    fn test_builder() {
        let rel = RelationBuilder::new(RelationId(99), 0)
            .from(blob(1))
            .to(blob(2))
            .kind(RelationKind::Snapshot)
            .weight(5)
            .build()
            .test_unwrap();
        assert_eq!(rel.rel_type.kind, RelationKind::Snapshot);
        assert_eq!(rel.rel_type.weight, RelationWeight(5));
    }

    #[test]
    fn test_builder_missing_from() {
        let err = RelationBuilder::new(RelationId(1), 0).to(blob(2)).build();
        assert!(err.is_err());
    }

    #[test]
    fn test_ondisk_size() {
        assert_eq!(core::mem::size_of::<RelationOnDisk>(), RELATION_ONDISK_SIZE);
    }

    #[test]
    fn test_from_on_disk_bad_kind() {
        let rel = Relation::new(
            RelationId(11),
            blob(0xCC),
            blob(0xDD),
            RelationType::new(RelationKind::Clone),
            50,
        );
        let mut on_disk = rel.to_on_disk();
        on_disk.kind = 0xFF; // invalid value
        assert!(Relation::from_on_disk(&on_disk).is_err());
    }

    #[test]
    fn test_summary() {
        let rel = Relation::new(
            RelationId(20),
            blob(2),
            blob(3),
            RelationType::new(RelationKind::Dedup),
            0,
        );
        let s = rel.summary();
        assert_eq!(s.id, RelationId(20));
        assert_eq!(s.kind, RelationKind::Dedup);
    }

    #[test]
    fn test_relation_id_valid() {
        assert!(!RelationId::INVALID.is_valid());
        assert!(RelationId(1).is_valid());
    }

    #[test]
    fn test_crc_validation() {
        let rel = Relation::new(
            RelationId(5),
            blob(0x10),
            blob(0x20),
            RelationType::new(RelationKind::HardLink),
            777,
        );
        let on_disk = rel.to_on_disk();
        let buf = on_disk.to_bytes();
        assert!(RelationOnDisk::crc_ok(&buf));
    }
}
