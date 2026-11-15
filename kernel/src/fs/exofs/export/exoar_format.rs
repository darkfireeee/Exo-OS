//! exoar_format.rs — ExoAR binary archive format (no_std, ExoFS-native).
//!
//! On-disk structure:
//!   ExoarHeader   (128 bytes) — global archive header
//!   [ ExoarEntryHeader (96 bytes) + payload bytes ] x N entries
//!   ExoarFooter   (32 bytes) — end magic + global CRC32 + counter
//!
//! RULE 8 : magic is validated FIRST in any read.
//! RULE 11: BlobId = blake3(raw data BEFORE compression/encryption).
//! ARITH-02: saturating_* / checked_* on all counters.
//! RECUR-01: no recursion.

use core::mem::size_of;

// ─── Magic constants ─────────────────────────────────────────────────────
/// ExoAR header magic: "EXOAR_AR" in little-endian.
pub const EXOAR_MAGIC: u64 = 0x4558_4F41_525F_4152;

/// Blob entry magic: "EXEN" in LE.
pub const EXOAR_ENTRY_MAGIC: u32 = 0x4558_454E;

/// Footer magic: "EXEO" in LE.
pub const EXOAR_FOOTER_MAGIC: u32 = 0x4558_454F;

/// Current version of ExoAR format.
pub const EXOAR_VERSION: u16 = 2;

/// Minimum size of a valid archive (header + footer alone).
pub const EXOAR_MIN_SIZE: usize = size_of::<ExoarHeader>() + size_of::<ExoarFooter>();

/// Maximum number of entries in an ExoAR archive.
pub const EXOAR_MAX_ENTRIES: u32 = 0x0010_0000;

/// Maximum entry payload size (256 MiB).
pub const EXOAR_MAX_PAYLOAD: u64 = 256 * 1024 * 1024;

#[inline]
fn pod_to_array<T: Copy, const N: usize>(value: &T) -> [u8; N] {
    let mut out = [0u8; N];
    if size_of::<T>() != N {
        return out;
    }
    // SAFETY: `out` has exactly the size of `T`; byte-by-byte copy without
    // constructing an aliased typed reference.
    unsafe {
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, out.as_mut_ptr(), N);
    }
    out
}

// ─── Archive flags ─────────────────────────────────────────────────────────
pub const ARCHIVE_FLAG_INCREMENTAL: u32 = 0x0001;
pub const ARCHIVE_FLAG_VERIFIED: u32 = 0x0002;
pub const ARCHIVE_FLAG_COMPRESSED: u32 = 0x0004;
pub const ARCHIVE_FLAG_ENCRYPTED: u32 = 0x0008;
pub const ARCHIVE_FLAG_SNAPSHOT: u32 = 0x0010;

// ─── Entry flags ──────────────────────────────────────────────────────────
pub const ENTRY_FLAG_COMPRESSED: u8 = 0x01;
pub const ENTRY_FLAG_ENCRYPTED: u8 = 0x02;
pub const ENTRY_FLAG_TOMBSTONE: u8 = 0x04;
pub const ENTRY_FLAG_HARDLINK: u8 = 0x08;
pub const ENTRY_FLAG_DIRECTORY: u8 = 0x10;
pub const ENTRY_FLAG_VERIFIED: u8 = 0x20;
pub const ENTRY_FLAG_FOREIGN: u8 = 0x40;

// ─── Compression algorithms ──────────────────────────────────────────────
pub const COMPRESS_NONE: u8 = 0;
pub const COMPRESS_LZ4: u8 = 1;
pub const COMPRESS_ZSTD: u8 = 2;
pub const COMPRESS_SNAPPY: u8 = 3;

// ─── Cipher algorithms ──────────────────────────────────────────────
pub const CIPHER_NONE: u8 = 0;
pub const CIPHER_AES256GCM: u8 = 1;
pub const CIPHER_CHACHA20: u8 = 2;

// ─── Binary structures ─────────────────────────────────────────────────────

/// Global ExoAR archive header (128 bytes, packed).
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct ExoarHeader {
    /// Magic validated FIRST — RULE 8.
    pub magic: u64,
    pub version: u16,
    pub flags: u32,
    pub epoch_base: u64,
    pub epoch_target: u64,
    pub created_at: u64,
    pub session_uuid: [u8; 16],
    pub entry_count: u32,
    pub compress_algo: u8,
    pub cipher_algo: u8,
    pub header_hash: [u8; 32],
    pub _pad: [u8; 36],
}

const _HEADER_SIZE: () = assert!(
    size_of::<ExoarHeader>() == 128,
    "ExoarHeader ABI size changed — verifier compatibilite ExoAR"
);

impl ExoarHeader {
    /// Creates a valid header with magic and version.
    pub const fn new(flags: u32, epoch_base: u64, epoch_target: u64) -> Self {
        Self {
            magic: EXOAR_MAGIC,
            version: EXOAR_VERSION,
            flags,
            epoch_base,
            epoch_target,
            created_at: 0,
            session_uuid: [0u8; 16],
            entry_count: 0,
            compress_algo: COMPRESS_NONE,
            cipher_algo: CIPHER_NONE,
            header_hash: [0u8; 32],
            _pad: [0u8; 36],
        }
    }

    /// Validates magic — RULE 8: FIRST.
    #[inline]
    pub fn validate_magic(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let m: u64 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.magic)) };
        m == EXOAR_MAGIC
    }

    /// Validates format version (1 <= version <= EXOAR_VERSION).
    #[inline]
    pub fn validate_version(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let v: u16 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.version)) };
        v >= 1 && v <= EXOAR_VERSION
    }

    /// Complete validation: magic + version + entry_count.
    pub fn validate(&self) -> bool {
        if !self.validate_magic() {
            return false;
        }
        if !self.validate_version() {
            return false;
        }
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let ec: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.entry_count)) };
        ec <= EXOAR_MAX_ENTRIES
    }

    /// Returns true if archive is incremental.
    #[inline]
    pub fn is_incremental(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let f: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.flags)) };
        f & ARCHIVE_FLAG_INCREMENTAL != 0
    }

    /// Serializes to bytes.
    pub fn as_bytes(&self) -> [u8; 128] {
        pod_to_array::<Self, 128>(self)
    }

    /// Deserializes from a byte slice (magic FIRST).
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < size_of::<Self>() {
            return None;
        }
        // SAFETY: safety invariant verified by preconditions of caller.
        let hdr: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        if !hdr.validate_magic() {
            return None;
        }
        Some(hdr)
    }
}

/// Blob entry header (96 bytes, packed).
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct ExoarEntryHeader {
    /// Magic validated FIRST — RULE 8.
    pub magic: u32,
    pub flags: u8,
    pub compress_algo: u8,
    /// BlobId = blake3(raw data) — RULE 11.
    pub blob_id: [u8; 32],
    pub payload_size: u64,
    pub original_size: u64,
    pub payload_crc32: u32,
    pub epoch: u64,
    pub name: [u8; 8],
    pub _reserved: [u8; 22],
}

const _ENTRY_SIZE: () = assert!(
    size_of::<ExoarEntryHeader>() == 96,
    "ExoarEntryHeader ABI size changed — verifier compatibilite ExoAR"
);

impl ExoarEntryHeader {
    /// Creates a valid entry header with magic.
    pub fn new(blob_id: [u8; 32], payload_size: u64, original_size: u64) -> Self {
        Self {
            magic: EXOAR_ENTRY_MAGIC,
            flags: 0,
            compress_algo: COMPRESS_NONE,
            blob_id,
            payload_size,
            original_size,
            payload_crc32: 0,
            epoch: 0,
            name: [0u8; 8],
            _reserved: [0u8; 22],
        }
    }

    /// Validates magic — RULE 8.
    #[inline]
    pub fn validate_magic(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let m: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.magic)) };
        m == EXOAR_ENTRY_MAGIC
    }

    /// Validates payload size <= EXOAR_MAX_PAYLOAD.
    #[inline]
    pub fn validate_size(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let ps: u64 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.payload_size)) };
        ps <= EXOAR_MAX_PAYLOAD
    }

    #[inline]
    pub fn is_tombstone(&self) -> bool {
        self.flags & ENTRY_FLAG_TOMBSTONE != 0
    }
    #[inline]
    pub fn is_compressed(&self) -> bool {
        self.flags & ENTRY_FLAG_COMPRESSED != 0
    }
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        self.flags & ENTRY_FLAG_ENCRYPTED != 0
    }
    #[inline]
    pub fn is_directory(&self) -> bool {
        self.flags & ENTRY_FLAG_DIRECTORY != 0
    }
    #[inline]
    pub fn is_hardlink(&self) -> bool {
        self.flags & ENTRY_FLAG_HARDLINK != 0
    }

    /// Serializes to bytes.
    pub fn as_bytes(&self) -> [u8; 96] {
        pod_to_array::<Self, 96>(self)
    }

    /// Deserializes from a slice (RULE 8: magic FIRST).
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < size_of::<Self>() {
            return None;
        }
        // SAFETY: safety invariant verified by preconditions of caller.
        let hdr: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        if !hdr.validate_magic() {
            return None;
        }
        if !hdr.validate_size() {
            return None;
        }
        Some(hdr)
    }
}

/// ExoAR archive footer (32 bytes, packed).
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct ExoarFooter {
    /// Magic validated FIRST — RULE 8.
    pub magic: u32,
    pub entry_count: u32,
    pub global_crc32: u32,
    pub total_size: u64,
    pub archive_hash: [u8; 8],
    pub _pad: [u8; 4],
}

const _FOOTER_SIZE: () = assert!(
    size_of::<ExoarFooter>() == 32,
    "ExoarFooter ABI size changed — verifier compatibilite ExoAR"
);

impl ExoarFooter {
    pub const fn new(entry_count: u32, global_crc32: u32, total_size: u64) -> Self {
        Self {
            magic: EXOAR_FOOTER_MAGIC,
            entry_count,
            global_crc32,
            total_size,
            archive_hash: [0u8; 8],
            _pad: [0u8; 4],
        }
    }

    /// Validates magic — RULE 8.
    #[inline]
    pub fn validate_magic(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let m: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.magic)) };
        m == EXOAR_FOOTER_MAGIC
    }

    pub fn validate(&self, expected_entries: u32) -> bool {
        if !self.validate_magic() {
            return false;
        }
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let ec: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.entry_count)) };
        ec == expected_entries
    }

    pub fn as_bytes(&self) -> [u8; 32] {
        pod_to_array::<Self, 32>(self)
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < size_of::<Self>() {
            return None;
        }
        // SAFETY: safety invariant verified by preconditions of caller.
        let ftr: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        if !ftr.validate_magic() {
            return None;
        }
        Some(ftr)
    }
}

/// Statistics summary of an ExoAR archive.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExoarSummary {
    pub entry_count: u32,
    pub tombstone_count: u32,
    pub compressed_count: u32,
    pub encrypted_count: u32,
    pub total_payload_bytes: u64,
    pub total_original_bytes: u64,
    pub crc_errors: u32,
    pub magic_errors: u32,
}

impl ExoarSummary {
    pub const fn new() -> Self {
        Self {
            entry_count: 0,
            tombstone_count: 0,
            compressed_count: 0,
            encrypted_count: 0,
            total_payload_bytes: 0,
            total_original_bytes: 0,
            crc_errors: 0,
            magic_errors: 0,
        }
    }

    /// Records an entry into summary.
    pub fn record_entry(&mut self, hdr: &ExoarEntryHeader, crc_ok: bool) {
        self.entry_count = self.entry_count.saturating_add(1);
        if hdr.is_tombstone() {
            self.tombstone_count = self.tombstone_count.saturating_add(1);
        }
        if hdr.is_compressed() {
            self.compressed_count = self.compressed_count.saturating_add(1);
        }
        if hdr.is_encrypted() {
            self.encrypted_count = self.encrypted_count.saturating_add(1);
        }
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let ps: u64 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.payload_size)) };
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let os: u64 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.original_size)) };
        self.total_payload_bytes = self.total_payload_bytes.saturating_add(ps);
        self.total_original_bytes = self.total_original_bytes.saturating_add(os);
        if !crc_ok {
            self.crc_errors = self.crc_errors.saturating_add(1);
        }
    }

    /// Compression ratio as percentage x 10 (avoids float). E.g.: 650 = 65.0%.
    pub fn compression_ratio_pct10(&self) -> u32 {
        if self.total_original_bytes == 0 {
            return 1000;
        }
        let ratio = (self.total_payload_bytes.saturating_mul(1000))
            .checked_div(self.total_original_bytes)
            .unwrap_or(1000);
        ratio.min(1000) as u32
    }

    #[inline]
    pub fn active_entry_count(&self) -> u32 {
        self.entry_count.saturating_sub(self.tombstone_count)
    }

    #[inline]
    pub fn is_clean(&self) -> bool {
        self.crc_errors == 0 && self.magic_errors == 0
    }
}

/// ExoAR entry kind, derived from its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoarEntryKind {
    Blob,
    Tombstone,
    Directory,
    HardLink,
    Foreign,
}

impl ExoarEntryKind {
    pub fn from_flags(flags: u8) -> Self {
        if flags & ENTRY_FLAG_TOMBSTONE != 0 {
            return Self::Tombstone;
        }
        if flags & ENTRY_FLAG_DIRECTORY != 0 {
            return Self::Directory;
        }
        if flags & ENTRY_FLAG_HARDLINK != 0 {
            return Self::HardLink;
        }
        if flags & ENTRY_FLAG_FOREIGN != 0 {
            return Self::Foreign;
        }
        Self::Blob
    }
}

/// ExoAR entry information after parsing.
#[derive(Clone, Copy, Debug)]
pub struct ExoarEntryInfo {
    pub kind: ExoarEntryKind,
    pub blob_id: [u8; 32],
    pub payload_size: u64,
    pub original_size: u64,
    pub declared_crc32: u32,
    pub epoch: u64,
    pub flags: u8,
}

impl ExoarEntryInfo {
    pub fn from_entry_header(hdr: &ExoarEntryHeader) -> Self {
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let payload_size =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.payload_size)) };
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let original_size =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.original_size)) };
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let declared_crc32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.payload_crc32)) };
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        let epoch = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.epoch)) };
        Self {
            kind: ExoarEntryKind::from_flags(hdr.flags),
            blob_id: hdr.blob_id,
            payload_size,
            original_size,
            declared_crc32,
            epoch,
            flags: hdr.flags,
        }
    }

    #[inline]
    pub fn has_payload(&self) -> bool {
        self.kind != ExoarEntryKind::Tombstone && self.payload_size > 0
    }
}

// ─── CRC32C (Castagnoli) ─────────────────────────────────────────────────────

/// CRC32C table precalculated at compile time.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            if crc & 1 != 0 {
                crc = 0x82F6_3B78 ^ (crc >> 1);
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Updates a CRC32C with additional data (RECUR-01: while loop).
pub fn crc32c_update(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    let mut i = 0usize;
    while i < data.len() {
        let idx = ((crc ^ (data[i] as u32)) & 0xFF) as usize;
        crc = CRC32C_TABLE[idx] ^ (crc >> 8);
        i = i.wrapping_add(1);
    }
    !crc
}

#[inline]
pub fn crc32c_compute(data: &[u8]) -> u32 {
    crc32c_update(0, data)
}
#[inline]
pub fn crc32c_verify(data: &[u8], expected: u32) -> bool {
    crc32c_compute(data) == expected
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_struct_sizes() {
        assert_eq!(size_of::<ExoarHeader>(), 128);
        assert_eq!(size_of::<ExoarEntryHeader>(), 96);
        assert_eq!(size_of::<ExoarFooter>(), 32);
    }

    #[test]
    fn test_header_new_magic() {
        let hdr = ExoarHeader::new(0, 1, 2);
        assert!(hdr.validate_magic());
        assert!(hdr.validate_version());
        assert!(hdr.validate());
    }

    #[test]
    fn test_header_bad_magic() {
        let mut hdr = ExoarHeader::new(0, 0, 0);
        hdr.magic = 0xDEAD_BEEF_DEAD_BEEF;
        assert!(!hdr.validate_magic());
        assert!(!hdr.validate());
    }

    #[test]
    fn test_entry_header_new() {
        let blob_id = [1u8; 32];
        let hdr = ExoarEntryHeader::new(blob_id, 1024, 2048);
        assert!(hdr.validate_magic());
        assert!(hdr.validate_size());
        assert_eq!(hdr.blob_id, blob_id);
    }

    #[test]
    fn test_entry_header_payload_limit() {
        let mut hdr = ExoarEntryHeader::new([0u8; 32], 0, 0);
        hdr.payload_size = EXOAR_MAX_PAYLOAD + 1;
        assert!(!hdr.validate_size());
    }

    #[test]
    fn test_footer_validate() {
        let ftr = ExoarFooter::new(42, 0xABCD, 8192);
        assert!(ftr.validate_magic());
        assert!(ftr.validate(42));
        assert!(!ftr.validate(41));
    }

    #[test]
    fn test_crc32c_basic() {
        let data = b"exofs rocks";
        let crc = crc32c_compute(data);
        assert_ne!(crc, 0);
        assert!(crc32c_verify(data, crc));
    }

    #[test]
    fn test_crc32c_incremental_eq_full() {
        let data = b"hello world exofs";
        let full = crc32c_compute(data);
        let p1 = crc32c_update(0, &data[..8]);
        let p2 = crc32c_update(p1, &data[8..]);
        assert_eq!(full, p2);
    }

    #[test]
    fn test_summary_record() {
        let mut s = ExoarSummary::new();
        let mut hdr = ExoarEntryHeader::new([0u8; 32], 100, 200);
        s.record_entry(&hdr, true);
        assert_eq!(s.entry_count, 1);
        assert_eq!(s.active_entry_count(), 1);

        hdr.flags |= ENTRY_FLAG_TOMBSTONE;
        s.record_entry(&hdr, false);
        assert_eq!(s.tombstone_count, 1);
        assert_eq!(s.crc_errors, 1);
        assert!(!s.is_clean());
    }

    #[test]
    fn test_compression_ratio() {
        let mut s = ExoarSummary::new();
        s.total_payload_bytes = 700;
        s.total_original_bytes = 1000;
        assert_eq!(s.compression_ratio_pct10(), 700);
    }

    #[test]
    fn test_entry_kind_from_flags() {
        assert_eq!(
            ExoarEntryKind::from_flags(ENTRY_FLAG_TOMBSTONE),
            ExoarEntryKind::Tombstone
        );
        assert_eq!(
            ExoarEntryKind::from_flags(ENTRY_FLAG_DIRECTORY),
            ExoarEntryKind::Directory
        );
        assert_eq!(ExoarEntryKind::from_flags(0), ExoarEntryKind::Blob);
    }

    #[test]
    fn test_entry_info_has_payload() {
        let hdr = ExoarEntryHeader::new([0u8; 32], 512, 1024);
        let info = ExoarEntryInfo::from_entry_header(&hdr);
        assert!(info.has_payload());
        assert_eq!(info.kind, ExoarEntryKind::Blob);
    }

    #[test]
    fn test_from_bytes_roundtrip() {
        let blob_id = [5u8; 32];
        let hdr = ExoarEntryHeader::new(blob_id, 256, 512);
        let bytes = hdr.as_bytes();
        let parsed = ExoarEntryHeader::from_bytes(&bytes).test_expect("parse ok");
        assert_eq!(parsed.blob_id, blob_id);
        // SAFETY: buffer of sufficient length, verified before call, repr(C).
        assert_eq!(
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(parsed.payload_size)) },
            256u64
        );
    }

    #[test]
    fn test_from_bytes_bad_magic() {
        let mut buf = [0u8; 96];
        buf[0] = 0xFF;
        buf[1] = 0xFF;
        buf[2] = 0xFF;
        buf[3] = 0xFF;
        assert!(ExoarEntryHeader::from_bytes(&buf).is_none());
    }

    #[test]
    fn test_header_incremental_flag() {
        let hdr = ExoarHeader::new(ARCHIVE_FLAG_INCREMENTAL, 5, 10);
        assert!(hdr.is_incremental());
        let hdr2 = ExoarHeader::new(ARCHIVE_FLAG_SNAPSHOT, 0, 0);
        assert!(!hdr2.is_incremental());
    }

    #[test]
    fn test_footer_as_bytes_len() {
        let ftr = ExoarFooter::new(0, 0, 0);
        assert_eq!(ftr.as_bytes().len(), 32);
    }
}
