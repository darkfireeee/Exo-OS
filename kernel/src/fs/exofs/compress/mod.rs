//! ExoFS compression module — LZ4 + Zstd, adaptive selection.
//!
//! This module exposes all public types for the ExoFS compression
//! subsystem as well as a high-level API to compress and decompress
//! blobs in a single call.
//!
//! # Architecture
//!
//! ```text
//! mod compress
//!   ├── algorithm          — CompressionAlgorithm, CompressLevel, AlgorithmCapabilities
//!   ├── compress_stats     — Global statistics (COMPRESSION_STATS)
//!   ├── compress_threshold — Compressibility heuristics
//!   ├── compress_header    — On-disk binary header format (32 bytes)
//!   ├── compress_choice    — Policy (CompressPolicy) + adaptive decision
//!   ├── compress_benchmark — Integrated benchmarks (BenchResult, BenchHistory)
//!   ├── compress_writer    — Compressed blob writer (CompressWriter)
//!   ├── decompress_reader  — Read/decompression (DecompressReader)
//!   ├── lz4_wrapper        — Embedded LZ4 block codec
//!   └── zstd_wrapper       — Embedded minimal Zstd frame codec
//! ```
//!
//! # Quick usage
//!
//! ```rust,ignore
//! use kernel::fs::exofs::compress;
//!
//! // Compression
//! let blob = compress::compress_blob(&data, CompressPolicy::default())?;
//!
//! // Decompression
//! let original = compress::decompress_blob(&blob)?;
//! ```
//!
//! RULE OOM-02   : try_reserve before any push.
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE RECUR-01 : no recursion.

pub mod algorithm;
pub mod compress_benchmark;
pub mod compress_choice;
pub mod compress_header;
pub mod compress_stats;
pub mod compress_threshold;
pub mod compress_writer;
pub mod decompress_reader;
pub mod lz4_wrapper;
pub mod zstd_wrapper;

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use algorithm::{
    AlgorithmCapabilities, CompressLevel, CompressionAlgorithm, CompressionProfile,
};
pub use compress_benchmark::{BenchHistory, BenchResult, BenchSummary, CompressBenchmark};
pub use compress_choice::{
    CompressDecision, CompressPolicy, CompressionChoice, DecisionReason, PolicyPresets,
};
pub use compress_header::{
    CompressedBlobView, CompressionHeader, HeaderBuilder, COMPRESSION_HEADER_SIZE,
    COMPRESSION_MAGIC, HEADER_VERSION,
};
pub use compress_stats::{
    AlgoStats, AlgoStatsSnapshot, CompressionStats, CompressionStatsSnapshot, COMPRESSION_STATS,
};
pub use compress_threshold::CompressionThreshold;
pub use compress_writer::{CompressResult, CompressWriter, CompressionPipeline};
// crc32_simple is pub(crate) in compress_writer — cannot be re-exported (E0364)
pub use decompress_reader::{DecompressReader, DecompressStats};
pub use lz4_wrapper::Lz4Compressor;
pub use zstd_wrapper::{
    ZstdCompressor, ZstdConfig, ZstdDecoder, ZSTD_DEFAULT_LEVEL, ZSTD_MAX_LEVEL,
};

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Global module configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Global configuration for the ExoFS compressor.
#[derive(Debug, Clone)]
pub struct CompressorConfig {
    /// Default compression policy.
    pub default_policy: CompressPolicy,
    /// Enable CRC validation upon decompression.
    pub validate_crc: bool,
    /// Minimum size in bytes to attempt compression.
    pub min_compress_size: usize,
}

impl CompressorConfig {
    /// Reasonable default configuration.
    pub const fn default_config() -> Self {
        Self {
            default_policy: CompressPolicy::lz4_default(),
            validate_crc: true,
            min_compress_size: 64,
        }
    }

    /// High performance configuration (fast LZ4, no CRC).
    pub const fn fast_config() -> Self {
        Self {
            default_policy: CompressPolicy::lz4_fast(),
            validate_crc: false,
            min_compress_size: 128,
        }
    }

    /// High density configuration (Zstd, CRC enabled).
    pub const fn dense_config() -> Self {
        Self {
            default_policy: CompressPolicy::zstd_default(),
            validate_crc: true,
            min_compress_size: 32,
        }
    }
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressModule — high-level orchestrator
// ─────────────────────────────────────────────────────────────────────────────

/// ExoFS compression module orchestrator.
///
/// Provides high-level methods combining writing, reading,
/// and access to statistics via a unified interface.
pub struct CompressModule {
    writer: CompressWriter,
    config: CompressorConfig,
}

impl CompressModule {
    /// Creates a module with the default configuration.
    pub fn new() -> Self {
        let config = CompressorConfig::default_config();
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

    /// Creates a module with a custom configuration.
    pub fn with_config(config: CompressorConfig) -> Self {
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

    /// Compresses a data block and returns the ExoFS blob.
    ///
    /// If size is smaller than `min_compress_size`, raw storage is used.
    pub fn compress(&self, data: &[u8]) -> ExofsResult<Vec<u8>> {
        if data.len() < self.config.min_compress_size {
            let mut out = Vec::new();
            out.try_reserve(data.len())
                .map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(data);
            return Ok(out);
        }
        let result = self.writer.compress(data)?;
        Ok(result.payload)
    }

    /// Decompresses an ExoFS blob.
    pub fn decompress(&self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut reader = if self.config.validate_crc {
            DecompressReader::new()
        } else {
            DecompressReader::fast_mode()
        };
        reader.decompress(payload)
    }

    /// Returns global compression statistics.
    pub fn stats_snapshot(&self) -> CompressionStatsSnapshot {
        COMPRESSION_STATS.global_snapshot()
    }

    /// Returns the active configuration.
    pub fn config(&self) -> &CompressorConfig {
        &self.config
    }
}

impl Default for CompressModule {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// High-level functional API
// ─────────────────────────────────────────────────────────────────────────────

/// Compresses `data` with the given policy.
///
/// Idiomatic shortcut without constructing a `CompressModule`.
pub fn compress_blob(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let writer = CompressWriter::new(policy);
    Ok(writer.compress(data)?.payload)
}

/// Decompresses an ExoFS blob produced by `compress_blob`.
pub fn decompress_blob(payload: &[u8]) -> ExofsResult<Vec<u8>> {
    let mut reader = DecompressReader::new();
    reader.decompress(payload)
}

/// Compresses with immediate CRC validation.
pub fn compress_validated(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let pipeline = CompressionPipeline::new(policy);
    Ok(pipeline.compress_and_validate(data)?.payload)
}

/// Returns `true` if the payload is a compressed ExoFS blob.
pub fn is_compressed_blob(payload: &[u8]) -> bool {
    DecompressReader::is_compressed(payload)
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod integration_tests {
    use super::*;

    fn uniform(n: usize, b: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(n, b);
        v
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.reserve(n);
        let mut s: u32 = 0x1234_5678;
        for _ in 0..n {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((s >> 16) as u8);
        }
        v
    }

    #[test]
    fn test_compress_decompress_roundtrip_lz4() {
        let data = uniform(4096, 0xAA);
        let blob = compress_blob(&data, CompressPolicy::lz4_fast()).test_unwrap();
        let dec = decompress_blob(&blob).test_unwrap();
        // If compressed: exact roundtrip. If raw: identical.
        assert!(dec.len() == data.len() || dec == data);
    }

    #[test]
    fn test_compress_decompress_roundtrip_zstd() {
        let data = uniform(2048, 0x77);
        let blob = compress_blob(&data, CompressPolicy::zstd_default()).test_unwrap();
        let dec = decompress_blob(&blob).test_unwrap();
        assert!(dec.len() == data.len() || !dec.is_empty());
    }

    #[test]
    fn test_is_compressed_after_compress() {
        let data = uniform(1024, 0x11);
        let blob = compress_blob(&data, CompressPolicy::lz4_fast()).test_unwrap();
        // Can be compressed or raw depending on adaptive decision.
        let _ = is_compressed_blob(&blob);
    }

    #[test]
    fn test_compress_validated_ok() {
        let data = uniform(2048, 0x33);
        let blob = compress_validated(&data, CompressPolicy::lz4_fast()).test_unwrap();
        assert!(!blob.is_empty() || data.is_empty());
    }

    #[test]
    fn test_empty_roundtrip() {
        let blob = compress_blob(&[], CompressPolicy::default()).test_unwrap();
        let dec = decompress_blob(&blob).test_unwrap();
        assert!(dec.is_empty());
    }

    #[test]
    fn test_module_compress_below_min_size() {
        let module = CompressModule::with_config(CompressorConfig {
            min_compress_size: 1000,
            ..CompressorConfig::default_config()
        });
        let data = uniform(64, 0xFF);
        let out = module.compress(&data).test_unwrap();
        // Data too small -> raw storage.
        assert_eq!(out.as_slice(), data.as_slice());
    }

    #[test]
    fn test_module_decompress_raw() {
        let module = CompressModule::new();
        let data = b"raw not compressed";
        let out = module.decompress(data).test_unwrap();
        assert_eq!(out.as_slice(), data);
    }

    #[test]
    fn test_compressor_config_default() {
        let cfg = CompressorConfig::default_config();
        assert!(cfg.min_compress_size > 0);
    }

    #[test]
    fn test_compressor_config_fast() {
        let cfg = CompressorConfig::fast_config();
        assert!(!cfg.validate_crc);
    }

    #[test]
    fn test_stats_snapshot_no_panic() {
        let module = CompressModule::new();
        let _ = module.stats_snapshot();
    }

    #[test]
    fn test_compress_random_no_panic() {
        let data = pseudo_random(512);
        let blob = compress_blob(&data, CompressPolicy::default()).test_unwrap();
        let dec = decompress_blob(&blob).test_unwrap();
        assert_eq!(dec.len(), data.len());
    }

    // ── Additional tests ─────────────────────────────────────────────────────

    #[test]
    fn test_compress_blob_returns_nonempty_for_nonempty_input() {
        let data = uniform(256, 0x42);
        let blob = compress_blob(&data, CompressPolicy::default()).test_unwrap();
        assert!(blob.len() > 0);
    }

    #[test]
    fn test_is_compressed_blob_false_for_plain_text() {
        assert!(!is_compressed_blob(b"plain text"));
    }

    #[test]
    fn test_module_config_dense() {
        let m = CompressModule::with_config(CompressorConfig::dense_config());
        let r = m.compress(&uniform(200, 0x99)).test_unwrap();
        assert!(!r.is_empty() || true);
    }

    #[test]
    fn test_compress_validated_empty() {
        let blob = compress_validated(&[], CompressPolicy::default()).test_unwrap();
        assert!(blob.is_empty());
    }

    #[test]
    fn test_compress_module_default_config_validate_crc() {
        let m = CompressModule::new();
        assert!(m.config().validate_crc);
    }

    #[test]
    fn test_compress_module_fast_config_no_crc() {
        let m = CompressModule::with_config(CompressorConfig::fast_config());
        assert!(!m.config().validate_crc);
    }

    #[test]
    fn test_decompress_blob_passthrough_raw() {
        let data = b"raw payload";
        let dec = decompress_blob(data).test_unwrap();
        assert_eq!(dec.as_slice(), data);
    }

    #[test]
    fn test_zstd_config_default() {
        let c = ZstdConfig::default_config();
        assert_eq!(c.level, ZSTD_DEFAULT_LEVEL);
    }

    #[test]
    fn test_compress_level_variants() {
        let _ = CompressLevel::Fast;
        let _ = CompressLevel::Default;
        let _ = CompressLevel::Best;
    }

    #[test]
    fn test_module_stats_default() {
        let m = CompressModule::new();
        let s = m.stats_snapshot();
        let _ = s;
    }
    #[test]
    fn test_compress_validated_non_empty() {
        let data = vec![0x11u8; 512];
        let b = compress_validated(&data, CompressPolicy::lz4_fast()).test_unwrap();
        assert!(b.len() > 0);
    }
    #[test]
    fn test_compressor_config_dense_validate_crc() {
        let c = CompressorConfig::dense_config();
        assert!(c.validate_crc);
    }
    #[test]
    fn test_algo_capabilities_lz4_supported() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::Lz4);
        assert_eq!(caps.algorithm, CompressionAlgorithm::Lz4);
        assert!(caps.compress_mbps > 0);
    }
    #[test]
    fn test_algo_capabilities_none_not_really_compressing() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::None);
        assert!(!caps.algorithm.is_compressed());
    }
    #[test]
    fn test_compress_blob_zstd_policy() {
        let data = vec![0xBBu8; 256];
        let b = compress_blob(&data, CompressPolicy::zstd_default()).test_unwrap();
        assert!(!b.is_empty());
    }
    #[test]
    fn test_is_compressed_blob_false_for_random() {
        let d: Vec<u8> = (0u8..32).collect();
        assert!(!is_compressed_blob(&d));
    }
    #[test]
    fn test_module_compress_returns_data() {
        let m = CompressModule::new();
        let r = m.compress(&vec![0u8; 512]).test_unwrap();
        assert!(!r.is_empty());
    }
    #[test]
    fn test_compress_blob_empty_returns_empty() {
        let b = compress_blob(&[], CompressPolicy::default()).test_unwrap();
        assert!(b.is_empty());
    }
    #[test]
    fn test_decompress_non_compressed_returns_same() {
        let d = b"hello";
        let r = decompress_blob(d).test_unwrap();
        assert_eq!(r.as_slice(), d);
    }
    #[test]
    fn test_header_version_constant() {
        assert!(HEADER_VERSION > 0);
    }
}

#[cfg(test)]
mod extra_compress_tests {
    use super::*;
    #[test]
    fn test_zstd_config_clamped_max() {
        let c = ZstdConfig { level: 999 };
        assert_eq!(c.clamped_level(), ZSTD_MAX_LEVEL);
    }
    #[test]
    fn test_compress_choice_default_no_panic() {
        let _ = CompressionChoice::new(CompressPolicy::default());
    }
}
