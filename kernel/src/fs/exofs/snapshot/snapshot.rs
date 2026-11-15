//! snapshot.rs — Structure and identifier of an ExoFS snapshot
//!
//! Defines the on-disk representation (ONDISK-03: no AtomicXxx) and in-memory representation,
//! as well as serialization and verification operations (HDR-03).
//!
//! Spec rules:
//!   ONDISK-03: plain types only in #[repr(C)]
//!   HDR-03   : magic verified FIRST, then Blake3 checksum
//!   ARITH-02 : checked_add for all arithmetic

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────

/// On-disk magic of a snapshot: "SNAP"
pub const SNAPSHOT_MAGIC: u32 = 0x534E_4150;

/// Fixed size of the disk header (256 bytes — ONDISK-03)
pub const SNAPSHOT_HEADER_SIZE: usize = 256;

/// Current version
pub const SNAPSHOT_FORMAT_VERSION: u8 = 1;

/// Maximum length of a snapshot name.
///
/// This value is bounded by the 256-byte on-disk format.
pub const SNAPSHOT_NAME_LEN: usize = 112;

/// Maximum number of concurrent snapshots
pub const SNAPSHOT_MAX_COUNT: usize = 1024;

// ─────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────

pub mod flags {
    /// Read-only snapshot (immutable)
    pub const READONLY: u32 = 1 << 0;
    /// Protected against deletion
    pub const PROTECTED: u32 = 1 << 1;
    /// Associated with an ongoing network stream
    pub const STREAMING: u32 = 1 << 2;
    /// Quota explicitly defined
    pub const QUOTA_SET: u32 = 1 << 3;
    /// Mounted snapshot
    pub const MOUNTED: u32 = 1 << 4;
    /// Snapshot being restored
    pub const RESTORING: u32 = 1 << 5;
    /// Orphan snapshot (parent not found)
    pub const ORPHAN: u32 = 1 << 6;
    /// Incremental snapshot type
    pub const INCREMENTAL: u32 = 1 << 7;
}

// ─────────────────────────────────────────────────────────────
// Disk structure (ONDISK-03 — plain types only)
// ─────────────────────────────────────────────────────────────

/// On-disk header of a snapshot (SNAPSHOT_HEADER_SIZE = 256 bytes)
///
/// No AtomicXxx — ONDISK-03.
/// Magic verified first — HDR-03.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SnapshotHeaderDisk {
    /// Magic "SNAP" — verified FIRST (HDR-03)
    pub magic: u32,
    /// Flags (flags::*)
    pub flags: u32,
    /// Unique snapshot identifier
    pub id: u64,
    /// Creation epoch
    pub epoch_id: u64,
    /// Parent snapshot identifier (0 = root)
    pub parent_id: u64,
    /// Creation timestamp (ticks)
    pub created_at: u64,
    /// Number of blobs contained
    pub n_blobs: u64,
    /// Total data size (bytes)
    pub total_bytes: u64,
    /// Disk offset of the blob catalog (0 = inline)
    pub blob_catalog_offset: u64,
    /// Merkle root of blob ids (concatenated Blake3 — HASH-02)
    pub root_blob: [u8; 32],
    /// Disk size of the blob catalog
    pub blob_catalog_size: u32,
    /// Name length (in bytes)
    pub name_len: u16,
    /// Format version
    pub version: u8,
    /// Explicit padding to keep a stable 256-byte disk format.
    pub _pad0: [u8; 9],
    /// Snapshot name (UTF-8 null-padded)
    pub name: [u8; SNAPSHOT_NAME_LEN],
    /// Blake3 checksum over the first SNAPSHOT_HEADER_SIZE - 32 bytes
    pub checksum: [u8; 32],
}

const _SH_SIZE: () = assert!(
    core::mem::size_of::<SnapshotHeaderDisk>() == SNAPSHOT_HEADER_SIZE,
    "SnapshotHeaderDisk must be exactly 256 bytes"
);

impl SnapshotHeaderDisk {
    /// Computes the Blake3 checksum of the header (over the first 224 bytes)
    pub fn compute_checksum(&self) -> [u8; 32] {
        let body_len = SNAPSHOT_HEADER_SIZE - 32;
        let mut body = [0u8; SNAPSHOT_HEADER_SIZE - 32];
        // SAFETY: SnapshotHeaderDisk is #[repr(C)] and the size is verified
        // by const assert; we copy only the prefix covered by the hash.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self as *const Self as *const u8,
                body.as_mut_ptr(),
                body_len,
            );
        }
        blake3_hash(&body)
    }

    /// HDR-03: verifies magic FIRST, then checksum
    pub fn verify(&self) -> ExofsResult<()> {
        if self.magic != SNAPSHOT_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        if self.version != SNAPSHOT_FORMAT_VERSION {
            return Err(ExofsError::InvalidArgument);
        }
        let expected = self.compute_checksum();
        let mut diff: u8 = 0;
        for i in 0..32 {
            diff |= expected[i] ^ self.checksum[i];
        }
        if diff != 0 {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Injects the computed checksum
    pub fn finalize(&mut self) {
        self.checksum = self.compute_checksum();
    }

    /// Returns the raw byte array
    pub fn as_bytes(&self) -> [u8; SNAPSHOT_HEADER_SIZE] {
        let mut out = [0u8; SNAPSHOT_HEADER_SIZE];
        // SAFETY: SnapshotHeaderDisk is #[repr(C)] and `out` has the
        // fixed size validated by const assert.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self as *const Self as *const u8,
                out.as_mut_ptr(),
                SNAPSHOT_HEADER_SIZE,
            );
        }
        out
    }

    /// Parses from a raw buffer
    pub fn from_bytes(buf: &[u8]) -> ExofsResult<Self> {
        if buf.len() < SNAPSHOT_HEADER_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        // SAFETY: verified size, repr(C)
        let hdr: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        Ok(hdr)
    }
}

// ─────────────────────────────────────────────────────────────
// In-memory snapshot
// ─────────────────────────────────────────────────────────────

/// RAM representation of a snapshot
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Unique identifier
    pub id: SnapshotId,
    /// Creation epoch
    pub epoch_id: EpochId,
    /// Parent snapshot (None = root)
    pub parent_id: Option<SnapshotId>,
    /// Merkle root of blobs (HASH-02: computed on RAW data)
    pub root_blob: BlobId,
    /// Creation timestamp
    pub created_at: u64,
    /// Number of blobs
    pub n_blobs: u64,
    /// Total data size
    pub total_bytes: u64,
    /// Flags (flags::*)
    pub flags: u32,
    /// Blob catalog offset on disk
    pub blob_catalog_offset: DiskOffset,
    /// Catalog size
    pub blob_catalog_size: u32,
    /// Name (null-padded)
    pub name: [u8; SNAPSHOT_NAME_LEN],
}

impl Snapshot {
    // ── Flag accessors ───────────────────────────────────────────

    pub fn is_readonly(&self) -> bool {
        self.flags & flags::READONLY != 0
    }
    pub fn is_protected(&self) -> bool {
        self.flags & flags::PROTECTED != 0
    }
    pub fn is_streaming(&self) -> bool {
        self.flags & flags::STREAMING != 0
    }
    pub fn is_mounted(&self) -> bool {
        self.flags & flags::MOUNTED != 0
    }
    pub fn is_restoring(&self) -> bool {
        self.flags & flags::RESTORING != 0
    }
    pub fn is_orphan(&self) -> bool {
        self.flags & flags::ORPHAN != 0
    }
    pub fn is_incremental(&self) -> bool {
        self.flags & flags::INCREMENTAL != 0
    }

    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }
    pub fn clear_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }
    pub fn toggle_flag(&mut self, flag: u32) {
        self.flags ^= flag;
    }

    // ── Name ──────────────────────────────────────────────────────────

    /// Returns the name as &str (UTF-8)
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(SNAPSHOT_NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    /// Effective length of the name
    pub fn name_len(&self) -> u16 {
        self.name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(SNAPSHOT_NAME_LEN) as u16
    }

    // ── Disk conversion ─────────────────────────────────────────────

    /// Constructs the corresponding disk header
    pub fn to_header_disk(&self) -> SnapshotHeaderDisk {
        let mut hdr = SnapshotHeaderDisk {
            magic: SNAPSHOT_MAGIC,
            flags: self.flags,
            id: self.id.0,
            epoch_id: self.epoch_id.0,
            parent_id: self.parent_id.map_or(0, |p| p.0),
            created_at: self.created_at,
            n_blobs: self.n_blobs,
            total_bytes: self.total_bytes,
            blob_catalog_offset: self.blob_catalog_offset.0,
            root_blob: *self.root_blob.as_bytes(),
            blob_catalog_size: self.blob_catalog_size,
            name_len: self.name_len(),
            version: SNAPSHOT_FORMAT_VERSION,
            _pad0: [0u8; 9],
            name: [0u8; SNAPSHOT_NAME_LEN],
            checksum: [0u8; 32],
        };
        hdr.name.copy_from_slice(&self.name);
        hdr.finalize();
        hdr
    }

    /// Deserializes from a disk header (HDR-03: magic + checksum verified)
    pub fn from_header_disk(hdr: &SnapshotHeaderDisk) -> ExofsResult<Self> {
        hdr.verify()?;

        let parent_id = if hdr.parent_id == 0 {
            None
        } else {
            Some(SnapshotId(hdr.parent_id))
        };

        let mut name = [0u8; SNAPSHOT_NAME_LEN];
        name.copy_from_slice(&hdr.name);

        Ok(Self {
            id: SnapshotId(hdr.id),
            epoch_id: EpochId(hdr.epoch_id),
            parent_id,
            root_blob: BlobId(hdr.root_blob),
            created_at: hdr.created_at,
            n_blobs: hdr.n_blobs,
            total_bytes: hdr.total_bytes,
            flags: hdr.flags,
            blob_catalog_offset: DiskOffset(hdr.blob_catalog_offset),
            blob_catalog_size: hdr.blob_catalog_size,
            name,
        })
    }

    // ── Stats utilities ─────────────────────────────────────────────

    /// Age in ticks relative to `now`
    pub fn age_ticks(&self, now: u64) -> u64 {
        now.saturating_sub(self.created_at)
    }

    /// Returns true if this snapshot is a descendant of `ancestor_id`
    pub fn has_parent(&self, ancestor_id: SnapshotId) -> bool {
        self.parent_id == Some(ancestor_id)
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotChain — ancestor chain
// ─────────────────────────────────────────────────────────────

/// Represents the chain of snapshots leading to a given snapshot
#[derive(Debug)]
pub struct SnapshotChain {
    pub ids: Vec<SnapshotId>,
}

impl SnapshotChain {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn root(&self) -> Option<SnapshotId> {
        self.ids.last().copied()
    }
    pub fn tip(&self) -> Option<SnapshotId> {
        self.ids.first().copied()
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotRef — lightweight reference (no full clone)
// ─────────────────────────────────────────────────────────────

/// Lightweight reference to the essential metadata of a snapshot
#[derive(Debug, Clone, Copy)]
pub struct SnapshotRef {
    pub id: SnapshotId,
    pub epoch_id: EpochId,
    pub parent_id: Option<SnapshotId>,
    pub n_blobs: u64,
    pub total_bytes: u64,
    pub flags: u32,
    pub created_at: u64,
}

impl From<&Snapshot> for SnapshotRef {
    fn from(s: &Snapshot) -> Self {
        Self {
            id: s.id,
            epoch_id: s.epoch_id,
            parent_id: s.parent_id,
            n_blobs: s.n_blobs,
            total_bytes: s.total_bytes,
            flags: s.flags,
            created_at: s.created_at,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────

/// Verifies a buffer as a snapshot header (HDR-03)
pub fn verify_snapshot_header(buf: &[u8]) -> ExofsResult<SnapshotHeaderDisk> {
    let hdr = SnapshotHeaderDisk::from_bytes(buf)?;
    hdr.verify()?;
    Ok(hdr)
}

/// Constructs a snapshot name from a slice (null-padded)
pub fn make_snapshot_name(src: &[u8]) -> [u8; SNAPSHOT_NAME_LEN] {
    let mut name = [0u8; SNAPSHOT_NAME_LEN];
    let len = src.len().min(SNAPSHOT_NAME_LEN);
    name[..len].copy_from_slice(&src[..len]);
    name
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_disk_size() {
        assert_eq!(
            core::mem::size_of::<SnapshotHeaderDisk>(),
            SNAPSHOT_HEADER_SIZE
        );
    }

    #[test]
    fn roundtrip_header_checksum() {
        let snap = Snapshot {
            id: SnapshotId(42),
            epoch_id: EpochId(7),
            parent_id: None,
            root_blob: BlobId([0xAB; 32]),
            created_at: 1234567,
            n_blobs: 10,
            total_bytes: 4096 * 10,
            flags: flags::READONLY,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"test-snap"),
        };
        let hdr = snap.to_header_disk();
        assert!(hdr.verify().is_ok());
    }

    #[test]
    fn bad_magic_detected() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: [0u8; SNAPSHOT_NAME_LEN],
        };
        let mut hdr = snap.to_header_disk();
        hdr.magic = 0xDEAD_BEEF;
        assert!(matches!(hdr.verify(), Err(ExofsError::BadMagic)));
    }

    #[test]
    fn checksum_mismatch_detected() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: [0u8; SNAPSHOT_NAME_LEN],
        };
        let mut hdr = snap.to_header_disk();
        hdr.n_blobs = 999; // Modify without recalculating checksum
        assert!(matches!(hdr.verify(), Err(ExofsError::ChecksumMismatch)));
    }

    #[test]
    fn from_header_disk_roundtrip() {
        let snap = Snapshot {
            id: SnapshotId(99),
            epoch_id: EpochId(5),
            parent_id: Some(SnapshotId(1)),
            root_blob: BlobId([0xCC; 32]),
            created_at: 88888,
            n_blobs: 5,
            total_bytes: 20480,
            flags: flags::PROTECTED | flags::READONLY,
            blob_catalog_offset: DiskOffset(4096),
            blob_catalog_size: 256,
            name: make_snapshot_name(b"prod-snapshot"),
        };
        let hdr = snap.to_header_disk();
        let snap2 = Snapshot::from_header_disk(&hdr).test_unwrap();
        assert_eq!(snap2.id.0, snap.id.0);
        assert_eq!(snap2.n_blobs, snap.n_blobs);
        assert_eq!(snap2.parent_id, snap.parent_id);
        assert!(snap2.is_protected());
    }

    #[test]
    fn name_str_valid() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"hello"),
        };
        assert_eq!(snap.name_str(), "hello");
    }
}
