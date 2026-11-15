//! ExoFS on-disk compression header.
//!
//! Persistent 32-byte structure preceding each compressed blob.
//!
//! RULE ONDISK-03 : #[repr(C)], asserted size, no AtomicU64.
//! RULE ARITH-02  : checked/saturating arithmetic only.
//! RULE MAGIC-01  : magic verification FIRST in all on-disk parsing.

use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// ExoFS compression header magic ("CMPR" little-endian).
pub const COMPRESSION_MAGIC: u32 = 0xC0_4D_50_52;

/// Current header format version.
pub const HEADER_VERSION: u8 = 1;

/// Fixed on-disk header size.
pub const COMPRESSION_HEADER_SIZE: usize = 32;

/// On-disk header preceding compressed data of an ExoFS blob.
///
/// Memory layout (32 bytes):
/// ```text
/// offset  size  field
///  0       4    magic
///  4       1    algorithm
///  5       1    level
///  6       1    version
///  7       1    flags
///  8       8    original_size
/// 16       8    compressed_size
/// 24       4    crc32
/// 28       4    _reserved
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CompressionHeader {
    /// Magic: must be COMPRESSION_MAGIC. Verified FIRST.
    pub magic: u32,
    /// Algorithm used (CompressionAlgorithm as u8).
    pub algorithm: u8,
    /// Compression level.
    pub level: u8,
    /// Header format version.
    pub version: u8,
    /// Flags reserved for future use.
    pub flags: u8,
    /// Decompressed data size in bytes.
    pub original_size: u64,
    /// Compressed data size (without this header).
    pub compressed_size: u64,
    /// CRC32 checksum of compressed data.
    pub crc32: u32,
    /// Reserved for future alignment.
    pub _reserved: [u8; 4],
}

// Static on-disk size check (ONDISK-03).
const _: () = assert!(
    core::mem::size_of::<CompressionHeader>() == COMPRESSION_HEADER_SIZE,
    "CompressionHeader must be exactly 32 bytes"
);

impl CompressionHeader {
    /// Creates a valid header with given parameters.
    pub fn new(
        algorithm: CompressionAlgorithm,
        level: u8,
        original_size: u64,
        compressed_size: u64,
        crc32: u32,
    ) -> Self {
        Self {
            magic: COMPRESSION_MAGIC,
            algorithm: algorithm as u8,
            level,
            version: HEADER_VERSION,
            flags: 0,
            original_size,
            compressed_size,
            crc32,
            _reserved: [0; 4],
        }
    }

    /// Parses from an on-disk buffer. MAGIC-01: magic verified FIRST.
    pub fn from_bytes(buf: &[u8]) -> ExofsResult<Self> {
        if buf.len() < COMPRESSION_HEADER_SIZE {
            return Err(ExofsError::CorruptedStructure);
        }
        // MAGIC-01: magic FIRST before any other field.
        let magic = u32::from_le_bytes(
            buf[0..4]
                .try_into()
                .map_err(|_| ExofsError::CorruptedStructure)?,
        );
        if magic != COMPRESSION_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // Check algorithm.
        let algo_byte = buf[4];
        if CompressionAlgorithm::from_u8(algo_byte).is_none() {
            return Err(ExofsError::NotSupported);
        }
        // Check version.
        let version = buf[6];
        if version > HEADER_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        // SAFETY: buf is sufficiently sized, repr(C) Plain Old Data.
        let header: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        Ok(header)
    }

    /// Serializes to on-disk bytes (32 bytes).
    ///
    /// SAFETY: Self is #[repr(C)], Plain Old Data, asserted size 32.
    pub fn to_bytes(&self) -> [u8; COMPRESSION_HEADER_SIZE] {
        let mut out = [0u8; COMPRESSION_HEADER_SIZE];
        // SAFETY: CompressionHeader is #[repr(C)] and size is verified by const assert.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self as *const Self as *const u8,
                out.as_mut_ptr(),
                COMPRESSION_HEADER_SIZE,
            );
        }
        out
    }

    /// Returns parsed algorithm.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::from_u8(self.algorithm).unwrap_or(CompressionAlgorithm::None)
    }

    /// `true` if header is structurally valid.
    pub fn is_valid(&self) -> bool {
        self.magic == COMPRESSION_MAGIC
            && CompressionAlgorithm::from_u8(self.algorithm).is_some()
            && self.version <= HEADER_VERSION
    }

    /// Verifies that `compressed_size` matches the provided payload buffer.
    pub fn validate_payload_len(&self, payload_len: usize) -> ExofsResult<()> {
        let expected = self.compressed_size as usize;
        if payload_len < expected {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(())
    }

    /// Total on-disk size (header + data).
    /// Checked arithmetic (ARITH-02).
    pub fn total_on_disk_size(&self) -> Option<u64> {
        (COMPRESSION_HEADER_SIZE as u64).checked_add(self.compressed_size)
    }

    /// `true` if data is compressed (algorithm != None).
    pub fn is_compressed(&self) -> bool {
        self.algorithm() != CompressionAlgorithm::None
    }

    /// Returns payload bytes from a full buffer.
    pub fn payload<'a>(&self, full_buf: &'a [u8]) -> ExofsResult<&'a [u8]> {
        let start = COMPRESSION_HEADER_SIZE;
        let end = start
            .checked_add(self.compressed_size as usize)
            .ok_or(ExofsError::OffsetOverflow)?;
        full_buf
            .get(start..end)
            .ok_or(ExofsError::CorruptedStructure)
    }
}

impl Default for CompressionHeader {
    fn default() -> Self {
        Self::new(CompressionAlgorithm::None, 0, 0, 0, 0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressedBlobView — structured view of a compressed blob
// ─────────────────────────────────────────────────────────────────────────────

/// Structured view of a buffer (parsed header + borrowed payload).
pub struct CompressedBlobView<'a> {
    pub header: CompressionHeader,
    pub payload: &'a [u8],
}

impl<'a> CompressedBlobView<'a> {
    /// Parses from a complete buffer (header + payload).
    /// Validates magic, algorithm, and size consistency.
    pub fn parse(buf: &'a [u8]) -> ExofsResult<Self> {
        let header = CompressionHeader::from_bytes(buf)?;
        let payload = header.payload(buf)?;
        Ok(Self { header, payload })
    }

    /// Original data size (before compression).
    pub fn original_size(&self) -> u64 {
        self.header.original_size
    }

    /// Algorithm used for this blob.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        self.header.algorithm()
    }

    /// CRC32 checksum of compressed data.
    pub fn crc32(&self) -> u32 {
        self.header.crc32
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
    use alloc::vec;

    fn make_valid_header_bytes(compressed_size: u64) -> [u8; 32] {
        let h = CompressionHeader::new(
            CompressionAlgorithm::Lz4,
            1,
            1000,
            compressed_size,
            0xDEAD_BEEF,
        );
        h.to_bytes()
    }

    #[test]
    fn test_header_size_is_32() {
        assert_eq!(core::mem::size_of::<CompressionHeader>(), 32);
    }

    #[test]
    fn test_new_and_is_valid() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 1000, 400, 0);
        assert!(h.is_valid());
        assert_eq!(h.magic, COMPRESSION_MAGIC);
        assert_eq!(h.version, HEADER_VERSION);
    }

    #[test]
    fn test_roundtrip_to_from_bytes() {
        let h = CompressionHeader::new(CompressionAlgorithm::Zstd, 3, 2048, 512, 0xABCD);
        let bytes = h.to_bytes();
        let h2 = CompressionHeader::from_bytes(&bytes).test_unwrap();
        assert_eq!(h.magic, h2.magic);
        assert_eq!(h.algorithm, h2.algorithm);
        assert_eq!(h.original_size, h2.original_size);
        assert_eq!(h.compressed_size, h2.compressed_size);
        assert_eq!(h.crc32, h2.crc32);
    }

    #[test]
    fn test_from_bytes_bad_magic() {
        let mut buf = make_valid_header_bytes(400);
        buf[0] = 0xFF;
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::InvalidMagic)
        ));
    }

    #[test]
    fn test_from_bytes_bad_algorithm() {
        let mut buf = make_valid_header_bytes(400);
        buf[4] = 0xFF;
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::NotSupported)
        ));
    }

    #[test]
    fn test_from_bytes_too_short() {
        let buf = [0u8; 10];
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::CorruptedStructure)
        ));
    }

    #[test]
    fn test_algorithm_accessor() {
        let h = CompressionHeader::new(CompressionAlgorithm::Zstd, 3, 100, 50, 0);
        assert_eq!(h.algorithm(), CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_total_on_disk_size() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 1000, 400, 0);
        assert_eq!(h.total_on_disk_size(), Some(432));
    }

    #[test]
    fn test_total_on_disk_size_overflow() {
        let h = CompressionHeader::new(CompressionAlgorithm::None, 0, 0, u64::MAX, 0);
        assert_eq!(h.total_on_disk_size(), None);
    }

    #[test]
    fn test_is_compressed_none() {
        let h = CompressionHeader::new(CompressionAlgorithm::None, 0, 100, 100, 0);
        assert!(!h.is_compressed());
    }

    #[test]
    fn test_is_compressed_lz4() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert!(h.is_compressed());
    }

    #[test]
    fn test_validate_payload_len_ok() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert!(h.validate_payload_len(50).is_ok());
    }

    #[test]
    fn test_validate_payload_len_too_short() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert_eq!(
            h.validate_payload_len(49),
            Err(ExofsError::CorruptedStructure)
        );
    }

    #[test]
    fn test_blob_view_parse_valid() {
        let h_bytes = make_valid_header_bytes(10);
        let mut buf = vec![0u8; 32 + 10];
        buf[..32].copy_from_slice(&h_bytes);
        let view = CompressedBlobView::parse(&buf).test_unwrap();
        assert_eq!(view.algorithm(), CompressionAlgorithm::Lz4);
        assert_eq!(view.original_size(), 1000);
    }

    #[test]
    fn test_blob_view_parse_bad_magic() {
        let mut buf = vec![0u8; 32];
        buf[0] = 0xAA;
        match CompressedBlobView::parse(&buf) {
            Err(e) => assert_eq!(e, ExofsError::InvalidMagic),
            Ok(_) => panic!("expected InvalidMagic"),
        }
    }

    #[test]
    fn test_default_header_none_algo() {
        let h = CompressionHeader::default();
        assert_eq!(h.algorithm(), CompressionAlgorithm::None);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HeaderBuilder — fluent builder for tests and compression code
// ─────────────────────────────────────────────────────────────────────────────

/// Fluent builder for `CompressionHeader`.
pub struct HeaderBuilder {
    algorithm: CompressionAlgorithm,
    level: u8,
    original_size: u64,
    compressed_size: u64,
    crc32: u32,
}

impl HeaderBuilder {
    pub const fn new() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: 1,
            original_size: 0,
            compressed_size: 0,
            crc32: 0,
        }
    }

    pub const fn algorithm(mut self, a: CompressionAlgorithm) -> Self {
        self.algorithm = a;
        self
    }
    pub const fn level(mut self, l: u8) -> Self {
        self.level = l;
        self
    }
    pub const fn uncompressed(mut self, n: u64) -> Self {
        self.original_size = n;
        self
    }
    pub const fn compressed(mut self, n: u64) -> Self {
        self.compressed_size = n;
        self
    }
    pub const fn crc32(mut self, c: u32) -> Self {
        self.crc32 = c;
        self
    }

    pub fn build(self) -> CompressionHeader {
        CompressionHeader::new(
            self.algorithm,
            self.level,
            self.original_size,
            self.compressed_size,
            self.crc32,
        )
    }
}

impl Default for HeaderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod header_builder_tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let h = HeaderBuilder::new()
            .algorithm(CompressionAlgorithm::Zstd)
            .level(3)
            .uncompressed(2048)
            .compressed(512)
            .crc32(0xCAFE_BABE)
            .build();
        assert_eq!(h.algorithm(), CompressionAlgorithm::Zstd);
        assert_eq!(h.original_size, 2048);
        assert_eq!(h.compressed_size, 512);
        assert_eq!(h.crc32, 0xCAFE_BABE);
    }

    #[test]
    fn test_builder_roundtrip() {
        let h = HeaderBuilder::new()
            .algorithm(CompressionAlgorithm::Lz4)
            .uncompressed(100)
            .compressed(50)
            .build();
        let b = h.to_bytes();
        let h2 = CompressionHeader::from_bytes(&b).test_unwrap();
        assert!(h2.is_valid());
        assert_eq!(h2.algorithm(), CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_version_in_bytes() {
        let h = HeaderBuilder::new().build();
        let b = h.to_bytes();
        assert_eq!(b[6], HEADER_VERSION); // offset 6 = version
    }
}
