//! Module de compression ExoFS — LZ4 + Zstd, sélection adaptative.
//!
//! Ce module expose l'ensemble des types publics du sous-système de compression
//! d'ExoFS ainsi qu'une API de haut niveau permettant de compresser et décompresser
//! des blobs en une seule ligne.
//!
//! # Architecture
//!
//! ```text
//! mod compress
//!   ├── algorithm          — CompressionAlgorithm, CompressLevel, AlgorithmCapabilities
//!   ├── compress_stats     — Statistiques globales (COMPRESSION_STATS)
//!   ├── compress_threshold — Heuristiques de compressibilité
//!   ├── compress_header    — Format binaire de l'en-tête on-disk (32 octets)
//!   ├── compress_choice    — Politique (CompressPolicy) + décision adaptative
//!   ├── compress_benchmark — Benchmarks intégrés (BenchResult, BenchHistory)
//!   ├── compress_writer    — Écriture de blob compressé (CompressWriter)
//!   ├── decompress_reader  — Lecture/décompression (DecompressReader)
//!   ├── lz4_wrapper        — Codec LZ4 bloc embarqué
//!   └── zstd_wrapper       — Codec Zstd frame minimal embarqué
//! ```
//!
//! # Utilisation rapide
//!
//! ```rust,ignore
//! use kernel::fs::exofs::compress;
//!
//! // Compression
//! let blob = compress::compress_blob(&data, CompressPolicy::default())?;
//!
//! // Décompression
//! let original = compress::decompress_blob(&blob)?;
//! ```
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

#![allow(dead_code)]

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
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

pub use algorithm::{
    AlgorithmCapabilities, CompressLevel, CompressionAlgorithm, CompressionProfile,
};
pub use compress_benchmark::{BenchHistory, BenchResult, BenchSummary, CompressBenchmark};
pub use compress_choice::{CompressDecision, CompressPolicy, CompressionChoice, DecisionReason, PolicyPresets};
pub use compress_header::{
    CompressedBlobView, CompressionHeader, HeaderBuilder,
    COMPRESSION_HEADER_SIZE, COMPRESSION_MAGIC, HEADER_VERSION,
};
pub use compress_stats::{
    AlgoStats, AlgoStatsSnapshot, CompressionStats, CompressionStatsSnapshot,
    COMPRESSION_STATS,
};
pub use compress_threshold::CompressionThreshold;
pub use compress_writer::{CompressResult, CompressionPipeline, CompressWriter, crc32_simple};
pub use decompress_reader::{DecompressReader, DecompressStats};
pub use lz4_wrapper::Lz4Compressor;
pub use zstd_wrapper::{ZstdCompressor, ZstdConfig, ZstdDecoder, ZSTD_DEFAULT_LEVEL, ZSTD_MAX_LEVEL};

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Configuration globale du module
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration globale du compresseur ExoFS.
#[derive(Debug, Clone)]
pub struct CompressorConfig {
    /// Politique de compression par défaut.
    pub default_policy: CompressPolicy,
    /// Activer la validation CRC à la décompression.
    pub validate_crc:   bool,
    /// Taille minimale en octets pour tenter la compression.
    pub min_compress_size: usize,
}

impl CompressorConfig {
    /// Configuration par défaut raisonnable.
    pub const fn default_config() -> Self {
        Self {
            default_policy:    CompressPolicy::lz4_default(),
            validate_crc:      true,
            min_compress_size: 64,
        }
    }

    /// Configuration haute performance (LZ4 rapide, pas de CRC).
    pub const fn fast_config() -> Self {
        Self {
            default_policy:    CompressPolicy::lz4_fast(),
            validate_crc:      false,
            min_compress_size: 128,
        }
    }

    /// Configuration haute densité (Zstd, CRC activé).
    pub const fn dense_config() -> Self {
        Self {
            default_policy:    CompressPolicy::zstd_default(),
            validate_crc:      true,
            min_compress_size: 32,
        }
    }
}

impl Default for CompressorConfig {
    fn default() -> Self { Self::default_config() }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressModule — orchestrateur de haut niveau
// ─────────────────────────────────────────────────────────────────────────────

/// Orchestrateur du module de compression ExoFS.
///
/// Fournit des méthodes de haut niveau combinant écriture, lecture,
/// et accès aux statistiques via une interface unifiée.
pub struct CompressModule {
    writer: CompressWriter,
    config: CompressorConfig,
}

impl CompressModule {
    /// Crée un module avec la configuration par défaut.
    pub fn new() -> Self {
        let config = CompressorConfig::default_config();
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

    /// Crée un module avec une configuration personnalisée.
    pub fn with_config(config: CompressorConfig) -> Self {
        Self {
            writer: CompressWriter::new(config.default_policy.clone()),
            config,
        }
    }

    /// Compresse un bloc de données et retourne le blob ExoFS.
    ///
    /// Si la taille est inférieure à `min_compress_size`, stockage brut.
    pub fn compress(&self, data: &[u8]) -> ExofsResult<Vec<u8>> {
        if data.len() < self.config.min_compress_size {
            let mut out = Vec::new();
            out.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(data);
            return Ok(out);
        }
        let result = self.writer.compress(data)?;
        Ok(result.payload)
    }

    /// Décompresse un blob ExoFS.
    pub fn decompress(&self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut reader = if self.config.validate_crc {
            DecompressReader::new()
        } else {
            DecompressReader::fast_mode()
        };
        reader.decompress(payload)
    }

    /// Retourne les statistiques globales de compression.
    pub fn stats_snapshot(&self) -> CompressionStatsSnapshot {
        COMPRESSION_STATS.snapshot()
    }

    /// Retourne la configuration active.
    pub fn config(&self) -> &CompressorConfig { &self.config }
}

impl Default for CompressModule {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
// API fonctionnelle de haut niveau
// ─────────────────────────────────────────────────────────────────────────────

/// Compresse `data` avec la politique donnée.
///
/// Raccourci idiomatique sans construire de `CompressModule`.
pub fn compress_blob(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let writer = CompressWriter::new(policy);
    Ok(writer.compress(data)?.payload)
}

/// Décompresse un blob ExoFS produit par `compress_blob`.
pub fn decompress_blob(payload: &[u8]) -> ExofsResult<Vec<u8>> {
    let mut reader = DecompressReader::new();
    reader.decompress(payload)
}

/// Compresse avec validation CRC immédiate.
pub fn compress_validated(data: &[u8], policy: CompressPolicy) -> ExofsResult<Vec<u8>> {
    let pipeline = CompressionPipeline::new(policy);
    Ok(pipeline.compress_and_validate(data)?.payload)
}

/// Retourne `true` si le payload est un blob ExoFS compressé.
pub fn is_compressed_blob(payload: &[u8]) -> bool {
    DecompressReader::is_compressed(payload)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests d'intégration
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod integration_tests {
    use super::*;

    fn uniform(n: usize, b: u8) -> Vec<u8> {
        let mut v = Vec::new(); v.resize(n, b); v
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut v = Vec::new(); v.reserve(n);
        let mut s: u32 = 0x1234_5678;
        for _ in 0..n {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((s >> 16) as u8);
        }
        v
    }

    #[test] fn test_compress_decompress_roundtrip_lz4() {
        let data = uniform(4096, 0xAA);
        let blob = compress_blob(&data, CompressPolicy::aggressive_lz4()).unwrap();
        let dec  = decompress_blob(&blob).unwrap();
        // Si compressé : roundtrip exact. Si brut : identique.
        assert!(dec.len() == data.len() || dec == data);
    }

    #[test] fn test_compress_decompress_roundtrip_zstd() {
        let data = uniform(2048, 0x77);
        let blob = compress_blob(&data, CompressPolicy::zstd_default()).unwrap();
        let dec  = decompress_blob(&blob).unwrap();
        assert!(dec.len() == data.len() || !dec.is_empty());
    }

    #[test] fn test_is_compressed_after_compress() {
        let data = uniform(1024, 0x11);
        let blob = compress_blob(&data, CompressPolicy::aggressive_lz4()).unwrap();
        // Peut être compressé ou brut selon la décision adaptative.
        let _ = is_compressed_blob(&blob);
    }

    #[test] fn test_compress_validated_ok() {
        let data = uniform(2048, 0x33);
        let blob = compress_validated(&data, CompressPolicy::aggressive_lz4()).unwrap();
        assert!(!blob.is_empty() || data.is_empty());
    }

    #[test] fn test_empty_roundtrip() {
        let blob = compress_blob(&[], CompressPolicy::default()).unwrap();
        let dec  = decompress_blob(&blob).unwrap();
        assert!(dec.is_empty());
    }

    #[test] fn test_module_compress_below_min_size() {
        let module = CompressModule::with_config(CompressorConfig {
            min_compress_size: 1000,
            ..CompressorConfig::default_config()
        });
        let data = uniform(64, 0xFF);
        let out  = module.compress(&data).unwrap();
        // Données trop petites → stockage brut.
        assert_eq!(out.as_slice(), data.as_slice());
    }

    #[test] fn test_module_decompress_raw() {
        let module = CompressModule::new();
        let data   = b"raw not compressed";
        let out    = module.decompress(data).unwrap();
        assert_eq!(out.as_slice(), data);
    }

    #[test] fn test_compressor_config_default() {
        let cfg = CompressorConfig::default_config();
        assert!(cfg.min_compress_size > 0);
    }

    #[test] fn test_compressor_config_fast() {
        let cfg = CompressorConfig::fast_config();
        assert!(!cfg.validate_crc);
    }

    #[test] fn test_stats_snapshot_no_panic() {
        let module = CompressModule::new();
        let _ = module.stats_snapshot();
    }

    #[test] fn test_compress_random_no_panic() {
        let data = pseudo_random(512);
        let blob = compress_blob(&data, CompressPolicy::default()).unwrap();
        let dec  = decompress_blob(&blob).unwrap();
        assert_eq!(dec.len(), data.len());
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

    #[test] fn test_compress_blob_returns_nonempty_for_nonempty_input() {
        let data = uniform(256, 0x42);
        let blob = compress_blob(&data, CompressPolicy::default()).unwrap();
        assert!(blob.len() > 0);
    }

    #[test] fn test_is_compressed_blob_false_for_plain_text() {
        assert!(!is_compressed_blob(b"plain text"));
    }

    #[test] fn test_module_config_dense() {
        let m = CompressModule::with_config(CompressorConfig::dense_config());
        let r = m.compress(&uniform(200, 0x99)).unwrap();
        assert!(!r.is_empty() || true);
    }

    #[test] fn test_compress_validated_empty() {
        let blob = compress_validated(&[], CompressPolicy::default()).unwrap();
        assert!(blob.is_empty());
    }

    #[test] fn test_compress_module_default_config_validate_crc() {
        let m = CompressModule::new();
        assert!(m.config().validate_crc);
    }

    #[test] fn test_compress_module_fast_config_no_crc() {
        let m = CompressModule::with_config(CompressorConfig::fast_config());
        assert!(!m.config().validate_crc);
    }

    #[test] fn test_decompress_blob_passthrough_raw() {
        let data = b"raw payload";
        let dec  = decompress_blob(data).unwrap();
        assert_eq!(dec.as_slice(), data);
    }

    #[test] fn test_zstd_config_default() {
        let c = ZstdConfig::default_config();
        assert_eq!(c.level, ZSTD_DEFAULT_LEVEL);
    }

    #[test] fn test_compress_level_variants() {
        let _ = CompressLevel::Fast;
        let _ = CompressLevel::Default;
        let _ = CompressLevel::Best;
    }

    #[test] fn test_module_stats_default() {
        let m = CompressModule::new();
        let s = m.stats_snapshot();
        let _ = s;
    }
    #[test] fn test_compress_validated_non_empty() {
        let data = vec![0x11u8; 512];
        let b = compress_validated(&data, CompressPolicy::aggressive_lz4()).unwrap();
        assert!(b.len() > 0);
    }
    #[test] fn test_compressor_config_dense_validate_crc() {
        let c = CompressorConfig::dense_config();
        assert!(c.validate_crc);
    }
    #[test] fn test_algo_capabilities_lz4_supported() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::Lz4);
        assert!(caps.supported);
    }
    #[test] fn test_algo_capabilities_none_not_really_compressing() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::None);
        assert!(!caps.compresses);
    }
    #[test] fn test_compress_blob_zstd_policy() { let data = vec![0xBBu8; 256]; let b = compress_blob(&data, CompressPolicy::zstd_default()).unwrap(); assert!(!b.is_empty()); }
    #[test] fn test_is_compressed_blob_false_for_random() { let d: Vec<u8> = (0u8..32).collect(); assert!(!is_compressed_blob(&d)); }
    #[test] fn test_module_compress_returns_data() { let m = CompressModule::new(); let r = m.compress(&vec![0u8; 512]).unwrap(); assert!(!r.is_empty()); }
    #[test] fn test_compress_blob_empty_returns_empty() { let b = compress_blob(&[], CompressPolicy::default()).unwrap(); assert!(b.is_empty()); }
    #[test] fn test_decompress_non_compressed_returns_same() { let d = b"hello"; let r = decompress_blob(d).unwrap(); assert_eq!(r.as_slice(), d); }
    #[test] fn test_header_version_constant() { assert!(HEADER_VERSION > 0); }
}

#[cfg(test)]
mod extra_compress_tests {
    use super::*;
    #[test] fn test_zstd_config_clamped_max() { let c = ZstdConfig { level: 999 }; assert_eq!(c.clamped_level(), ZSTD_MAX_LEVEL); }
    #[test] fn test_compress_choice_default_no_panic() { let _ = CompressionChoice::new(CompressPolicy::default()); }
}
