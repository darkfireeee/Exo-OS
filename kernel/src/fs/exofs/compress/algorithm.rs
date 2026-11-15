//! Compression algorithms supported by ExoFS.
//!
//! Defines: algorithms, levels, buffer bounds, capabilities, validation.
//!
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE ONDISK-03: no AtomicU64 in #[repr(C)] types.

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// CompressLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Compression levels (1 = fast, 9 = maximum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CompressLevel {
    None = 0,
    Fast = 1,
    Default = 3,
    Best = 6,
    Maximum = 9,
}

impl CompressLevel {
    /// Constructs from an on-disk byte. Returns `Default` if invalid.
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Fast,
            3 => Self::Default,
            6 => Self::Best,
            7..=255 => Self::Maximum,
            _ => Self::Default,
        }
    }

    /// Numeric value for embedded library calls.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// `true` if level is at least `Default`.
    pub const fn is_at_least_default(self) -> bool {
        (self as u8) >= (CompressLevel::Default as u8)
    }

    /// Returns the immediately higher level (clamped to Maximum).
    pub const fn next(self) -> Self {
        match self {
            Self::None => Self::Fast,
            Self::Fast => Self::Default,
            Self::Default => Self::Best,
            Self::Best => Self::Maximum,
            Self::Maximum => Self::Maximum,
        }
    }

    /// Returns the immediately lower level (floored at Fast).
    pub const fn prev(self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Fast => Self::Fast,
            Self::Default => Self::Fast,
            Self::Best => Self::Default,
            Self::Maximum => Self::Best,
        }
    }

    /// Human-readable name of the level.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fast => "fast",
            Self::Default => "default",
            Self::Best => "best",
            Self::Maximum => "maximum",
        }
    }

    /// All valid levels, from fastest to slowest.
    pub const ALL: [CompressLevel; 4] = [
        CompressLevel::Fast,
        CompressLevel::Default,
        CompressLevel::Best,
        CompressLevel::Maximum,
    ];
}

impl Default for CompressLevel {
    fn default() -> Self {
        CompressLevel::Default
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionAlgorithm
// ─────────────────────────────────────────────────────────────────────────────

/// Compression algorithm used for a blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompressionAlgorithm {
    /// No compression.
    None = 0,
    /// LZ4 — fast, low ratio, ideal for hot data.
    Lz4 = 1,
    /// Zstd — better ratio, slightly slower.
    Zstd = 2,
}

impl CompressionAlgorithm {
    /// Returns the human-readable name of the algorithm.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }

    /// `true` if data is effectively compressed.
    pub const fn is_compressed(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Parses from an on-disk byte. Returns `None` if unknown value.
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            _ => None,
        }
    }

    /// Parses from an on-disk byte, returns `ExofsError::NotSupported` if unknown.
    pub fn parse_on_disk(v: u8) -> ExofsResult<Self> {
        Self::from_u8(v).ok_or(ExofsError::NotSupported)
    }

    /// Recommended default compression level for this algorithm.
    pub const fn default_level(self) -> CompressLevel {
        match self {
            Self::None => CompressLevel::Default,
            Self::Lz4 => CompressLevel::Fast,
            Self::Zstd => CompressLevel::Default,
        }
    }

    /// `true` if the algorithm supports multiple levels.
    pub const fn supports_levels(self) -> bool {
        matches!(self, Self::Zstd)
    }

    /// Relative latency (1 = fastest, reference memory copy).
    pub const fn relative_latency(self) -> u8 {
        match self {
            Self::None => 1,
            Self::Lz4 => 2,
            Self::Zstd => 5,
        }
    }

    /// All algorithms, including None.
    pub const ALL: [CompressionAlgorithm; 3] = [
        CompressionAlgorithm::None,
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Zstd,
    ];

    /// Compressing algorithms only (excluding None).
    pub const COMPRESSING: [CompressionAlgorithm; 2] =
        [CompressionAlgorithm::Lz4, CompressionAlgorithm::Zstd];
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        CompressionAlgorithm::None
    }
}
impl core::convert::TryFrom<u8> for CompressionAlgorithm {
    type Error = ();
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::from_u8(v).ok_or(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Buffer constants
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4: max output = input + input/255 + 16 bytes (block format specification).
pub const LZ4_MAX_EXPANSION_BYTES: usize = 16;

/// Zstd: additional safety margin outside strict specification.
pub const ZSTD_COMPRESSBOUND_MARGIN: usize = 128;

/// Maximum compressed block size accepted as input (anti-DoS).
pub const MAX_COMPRESSED_BLOCK_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// Maximum decompressed block size accepted (anti-DoS).
pub const MAX_DECOMPRESSED_BLOCK_SIZE: usize = 256 * 1024 * 1024; // 256 MiB

/// Minimum size for compression to be attempted.
pub const MIN_COMPRESS_INPUT: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Bound computation functions
// ─────────────────────────────────────────────────────────────────────────────

/// Computes upper bound for output buffer.
/// Saturating arithmetic (ARITH-02): never panics on overflow.
pub fn compress_bound(algorithm: CompressionAlgorithm, input_len: usize) -> usize {
    match algorithm {
        CompressionAlgorithm::None => input_len,
        CompressionAlgorithm::Lz4 => {
            let margin = input_len.saturating_div(255);
            input_len
                .saturating_add(margin)
                .saturating_add(LZ4_MAX_EXPANSION_BYTES)
        }
        CompressionAlgorithm::Zstd => input_len.saturating_add(ZSTD_COMPRESSBOUND_MARGIN),
    }
}

/// Validates that an uncompressed size is within safety limits.
pub fn validate_decompressed_size(uncompressed_size: u64) -> ExofsResult<usize> {
    let sz = uncompressed_size as usize;
    if sz > MAX_DECOMPRESSED_BLOCK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(sz)
}

/// Validates that a compressed size is within safety limits.
pub fn validate_compressed_size(compressed_size: u64) -> ExofsResult<usize> {
    let sz = compressed_size as usize;
    if sz > MAX_COMPRESSED_BLOCK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(sz)
}

// ─────────────────────────────────────────────────────────────────────────────
// AlgorithmCapabilities — declared capabilities per algorithm
// ─────────────────────────────────────────────────────────────────────────────

/// Capabilities of a compression algorithm.
#[derive(Debug, Clone, Copy)]
pub struct AlgorithmCapabilities {
    pub algorithm: CompressionAlgorithm,
    pub stream_support: bool,
    pub dict_support: bool,
    pub max_level: CompressLevel,
    /// Typical compression throughput in MB/s.
    pub compress_mbps: u32,
    /// Typical decompression throughput in MB/s.
    pub decompress_mbps: u32,
}

impl AlgorithmCapabilities {
    /// Returns capabilities for the given algorithm.
    pub const fn for_algorithm(algo: CompressionAlgorithm) -> Self {
        match algo {
            CompressionAlgorithm::None => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::None,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Default,
                compress_mbps: 10_000,
                decompress_mbps: 10_000,
            },
            CompressionAlgorithm::Lz4 => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::Lz4,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Fast,
                compress_mbps: 600,
                decompress_mbps: 3_000,
            },
            CompressionAlgorithm::Zstd => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::Zstd,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Maximum,
                compress_mbps: 400,
                decompress_mbps: 1_500,
            },
        }
    }

    /// `true` if the algorithm can be used for this level.
    pub fn level_supported(&self, level: CompressLevel) -> bool {
        level <= self.max_level
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionProfile — persistable (algorithm, level) pair
// ─────────────────────────────────────────────────────────────────────────────

/// Compression profile serializable on 2 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionProfile {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressLevel,
}

impl CompressionProfile {
    /// Default profile: Lz4 in Fast mode.
    pub const fn default_lz4() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Lz4,
            level: CompressLevel::Fast,
        }
    }

    /// High ratio profile: Zstd Default.
    pub const fn high_ratio() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            level: CompressLevel::Default,
        }
    }

    /// No compression.
    pub const fn none() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: CompressLevel::Default,
        }
    }

    /// Serializes to 2 bytes (algorithm, level).
    pub const fn to_bytes(self) -> [u8; 2] {
        [self.algorithm as u8, self.level as u8]
    }

    /// Deserializes from 2 bytes.
    pub fn from_bytes(b: [u8; 2]) -> ExofsResult<Self> {
        let algorithm = CompressionAlgorithm::parse_on_disk(b[0])?;
        let level = CompressLevel::from_u8(b[1]);
        Ok(Self { algorithm, level })
    }
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self::default_lz4()
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
    fn test_algorithm_from_u8_all() {
        assert_eq!(
            CompressionAlgorithm::from_u8(0),
            Some(CompressionAlgorithm::None)
        );
        assert_eq!(
            CompressionAlgorithm::from_u8(1),
            Some(CompressionAlgorithm::Lz4)
        );
        assert_eq!(
            CompressionAlgorithm::from_u8(2),
            Some(CompressionAlgorithm::Zstd)
        );
        assert_eq!(CompressionAlgorithm::from_u8(3), None);
        assert_eq!(CompressionAlgorithm::from_u8(255), None);
    }

    #[test]
    fn test_parse_on_disk_invalid_returns_not_supported() {
        assert_eq!(
            CompressionAlgorithm::parse_on_disk(42),
            Err(ExofsError::NotSupported)
        );
    }

    #[test]
    fn test_level_from_u8_edges() {
        assert_eq!(CompressLevel::from_u8(1), CompressLevel::Fast);
        assert_eq!(CompressLevel::from_u8(3), CompressLevel::Default);
        assert_eq!(CompressLevel::from_u8(6), CompressLevel::Best);
        assert_eq!(CompressLevel::from_u8(9), CompressLevel::Maximum);
        assert_eq!(CompressLevel::from_u8(0), CompressLevel::Default); // invalid → Default
    }

    #[test]
    fn test_compress_bound_lz4() {
        let n = compress_bound(CompressionAlgorithm::Lz4, 1000);
        assert!(n >= 1000 + 16);
    }

    #[test]
    fn test_compress_bound_none_identity() {
        assert_eq!(compress_bound(CompressionAlgorithm::None, 500), 500);
    }

    #[test]
    fn test_compress_bound_no_overflow() {
        // saturating_add: never panics even with max usize
        let n = compress_bound(CompressionAlgorithm::Zstd, usize::MAX - 256);
        assert!(n > 0);
    }

    #[test]
    fn test_validate_decompressed_size_ok() {
        assert!(validate_decompressed_size(1024).is_ok());
        assert_eq!(validate_decompressed_size(1024).test_unwrap(), 1024);
    }

    #[test]
    fn test_validate_decompressed_size_over_limit() {
        let over = (MAX_DECOMPRESSED_BLOCK_SIZE + 1) as u64;
        assert_eq!(
            validate_decompressed_size(over),
            Err(ExofsError::InvalidArgument)
        );
    }

    #[test]
    fn test_validate_compressed_size_over_limit() {
        let over = (MAX_COMPRESSED_BLOCK_SIZE + 1) as u64;
        assert_eq!(
            validate_compressed_size(over),
            Err(ExofsError::InvalidArgument)
        );
    }

    #[test]
    fn test_level_next_chain() {
        assert_eq!(CompressLevel::Fast.next(), CompressLevel::Default);
        assert_eq!(CompressLevel::Default.next(), CompressLevel::Best);
        assert_eq!(CompressLevel::Best.next(), CompressLevel::Maximum);
        assert_eq!(CompressLevel::Maximum.next(), CompressLevel::Maximum);
    }

    #[test]
    fn test_level_prev_chain() {
        assert_eq!(CompressLevel::Maximum.prev(), CompressLevel::Best);
        assert_eq!(CompressLevel::Best.prev(), CompressLevel::Default);
        assert_eq!(CompressLevel::Default.prev(), CompressLevel::Fast);
        assert_eq!(CompressLevel::Fast.prev(), CompressLevel::Fast);
    }

    #[test]
    fn test_capabilities_lz4_decompresses_faster() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::Lz4);
        assert!(caps.decompress_mbps > caps.compress_mbps);
    }

    #[test]
    fn test_algorithm_is_compressed() {
        assert!(!CompressionAlgorithm::None.is_compressed());
        assert!(CompressionAlgorithm::Lz4.is_compressed());
        assert!(CompressionAlgorithm::Zstd.is_compressed());
    }

    #[test]
    fn test_compression_profile_roundtrip() {
        let p = CompressionProfile::default_lz4();
        let b = p.to_bytes();
        let p2 = CompressionProfile::from_bytes(b).test_unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn test_compression_profile_high_ratio() {
        let p = CompressionProfile::high_ratio();
        assert_eq!(p.algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(p.level, CompressLevel::Default);
    }

    #[test]
    fn test_level_is_at_least_default() {
        assert!(!CompressLevel::Fast.is_at_least_default());
        assert!(CompressLevel::Default.is_at_least_default());
        assert!(CompressLevel::Maximum.is_at_least_default());
    }

    #[test]
    fn test_algorithm_supports_levels() {
        assert!(!CompressionAlgorithm::Lz4.supports_levels());
        assert!(CompressionAlgorithm::Zstd.supports_levels());
    }
}
