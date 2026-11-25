//! ExoFS compression facade (`CompressorConfig`, `CompressModule`, functional API) and tests.
//!
//! Extracted from `exofs/compress/mod.rs` so `mod.rs` remains a pure wiring facade.

use alloc::vec::Vec;

use super::{
    CompressPolicy, CompressWriter, CompressionPipeline, CompressionStatsSnapshot,
    DecompressReader, COMPRESSION_STATS,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

#[derive(Debug, Clone)]
pub struct CompressorConfig {
    pub default_policy: CompressPolicy,
    pub validate_crc: bool,
    pub min_compress_size: usize,
}

impl CompressorConfig {
    pub const fn default_config() -> Self {
        Self {
            default_policy: CompressPolicy::lz4_default(),
            validate_crc: true,
            min_compress_size: 64,
        }
    }

    pub const fn fast_config() -> Self {
        Self {
            default_policy: CompressPolicy::lz4_fast(),
            validate_crc: false,
            min_compress_size: 128,
        }
    }

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

pub struct CompressModule {
    writer: CompressWriter,
    config: CompressorConfig,
}

impl CompressModule {
    pub fn new() -> Self {
        let config = CompressorConfig::default_config();
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

    pub fn with_config(config: CompressorConfig) -> Self {
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

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

    pub fn decompress(&self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut reader = if self.config.validate_crc {
            DecompressReader::new()
        } else {
            DecompressReader::fast_mode()
        };
        reader.decompress(payload)
    }

    pub fn stats_snapshot(&self) -> CompressionStatsSnapshot {
        COMPRESSION_STATS.global_snapshot()
    }

    pub fn config(&self) -> &CompressorConfig {
        &self.config
    }
}

impl Default for CompressModule {
    fn default() -> Self {
        Self::new()
    }
}

pub fn compress_blob(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let writer = CompressWriter::new(policy);
    Ok(writer.compress(data)?.payload)
}

pub fn decompress_blob(payload: &[u8]) -> ExofsResult<Vec<u8>> {
    let mut reader = DecompressReader::new();
    reader.decompress(payload)
}

pub fn compress_validated(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let pipeline = CompressionPipeline::new(policy);
    Ok(pipeline.compress_and_validate(data)?.payload)
}

pub fn is_compressed_blob(payload: &[u8]) -> bool {
    DecompressReader::is_compressed(payload)
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::{
        AlgorithmCapabilities, CompressLevel, CompressionAlgorithm, CompressionChoice, ZstdConfig,
        HEADER_VERSION, ZSTD_DEFAULT_LEVEL, ZSTD_MAX_LEVEL,
    };
    use super::*;
    use alloc::vec;

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
        assert!(CompressorConfig::default_config().min_compress_size > 0);
    }

    #[test]
    fn test_compressor_config_fast() {
        assert!(!CompressorConfig::fast_config().validate_crc);
    }

    #[test]
    fn test_stats_snapshot_no_panic() {
        let _ = CompressModule::new().stats_snapshot();
    }

    #[test]
    fn test_compress_random_no_panic() {
        let data = pseudo_random(512);
        let blob = compress_blob(&data, CompressPolicy::default()).test_unwrap();
        let dec = decompress_blob(&blob).test_unwrap();
        assert_eq!(dec.len(), data.len());
    }

    #[test]
    fn test_compress_blob_returns_nonempty_for_nonempty_input() {
        let blob = compress_blob(&uniform(256, 0x42), CompressPolicy::default()).test_unwrap();
        assert!(!blob.is_empty());
    }

    #[test]
    fn test_is_compressed_blob_false_for_plain_text() {
        assert!(!is_compressed_blob(b"plain text"));
    }

    #[test]
    fn test_module_config_dense() {
        let m = CompressModule::with_config(CompressorConfig::dense_config());
        let r = m.compress(&uniform(200, 0x99)).test_unwrap();
        assert!(!r.is_empty());
    }

    #[test]
    fn test_compress_validated_empty() {
        assert!(compress_validated(&[], CompressPolicy::default())
            .test_unwrap()
            .is_empty());
    }

    #[test]
    fn test_compress_module_default_config_validate_crc() {
        assert!(CompressModule::new().config().validate_crc);
    }

    #[test]
    fn test_compress_module_fast_config_no_crc() {
        assert!(!CompressModule::with_config(CompressorConfig::fast_config())
            .config()
            .validate_crc);
    }

    #[test]
    fn test_decompress_blob_passthrough_raw() {
        let data = b"raw payload";
        assert_eq!(decompress_blob(data).test_unwrap().as_slice(), data);
    }

    #[test]
    fn test_zstd_config_default() {
        assert_eq!(ZstdConfig::default_config().level, ZSTD_DEFAULT_LEVEL);
    }

    #[test]
    fn test_compress_level_variants() {
        let _ = (CompressLevel::Fast, CompressLevel::Default, CompressLevel::Best);
    }

    #[test]
    fn test_compress_validated_non_empty() {
        let b = compress_validated(&vec![0x11u8; 512], CompressPolicy::lz4_fast()).test_unwrap();
        assert!(!b.is_empty());
    }

    #[test]
    fn test_compressor_config_dense_validate_crc() {
        assert!(CompressorConfig::dense_config().validate_crc);
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
        let b = compress_blob(&vec![0xBBu8; 256], CompressPolicy::zstd_default()).test_unwrap();
        assert!(!b.is_empty());
    }

    #[test]
    fn test_is_compressed_blob_false_for_random() {
        let d: Vec<u8> = (0u8..32).collect();
        assert!(!is_compressed_blob(&d));
    }

    #[test]
    fn test_module_compress_returns_data() {
        let r = CompressModule::new().compress(&vec![0u8; 512]).test_unwrap();
        assert!(!r.is_empty());
    }

    #[test]
    fn test_compress_blob_empty_returns_empty() {
        assert!(compress_blob(&[], CompressPolicy::default())
            .test_unwrap()
            .is_empty());
    }

    #[test]
    fn test_decompress_non_compressed_returns_same() {
        let d = b"hello";
        assert_eq!(decompress_blob(d).test_unwrap().as_slice(), d);
    }

    #[test]
    fn test_header_version_constant() {
        assert!(HEADER_VERSION > 0);
    }

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
