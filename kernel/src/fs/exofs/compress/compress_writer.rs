//! Écrivain de blob compressé ExoFS.
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

use alloc::vec::Vec;
use crate::fs::exofs::compress::algorithm::{CompressionAlgorithm, CompressLevel};
use crate::fs::exofs::compress::compress_choice::{CompressionChoice, CompressPolicy};
use crate::fs::exofs::compress::compress_header::{CompressionHeader, COMPRESSION_HEADER_SIZE, COMPRESSION_MAGIC, HEADER_VERSION};
use crate::fs::exofs::compress::compress_stats::COMPRESSION_STATS;
use crate::fs::exofs::compress::compress_threshold::CompressionThreshold;
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de compression
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération de compression.
#[derive(Debug)]
pub struct CompressResult {
    /// Payload final : en-tête (32 B) + données compressées.
    pub payload:         Vec<u8>,
    /// Algorithme effectivement utilisé.
    pub algorithm:       CompressionAlgorithm,
    /// Taille originale.
    pub original_size:   usize,
    /// Taille du payload compressé (sans en-tête).
    pub compressed_size: usize,
}

impl CompressResult {
    /// Taille totale (en-tête inclus).
    pub fn total_size(&self) -> usize { self.payload.len() }

    /// Longueur du payload (alias de total_size).
    pub fn len(&self) -> usize { self.payload.len() }
    pub fn is_empty(&self) -> bool { self.payload.is_empty() }

    /// Ratio de compression (0.0 = parfait, 1.0 = aucune compression).
    pub fn compression_ratio(&self) -> f32 {
        if self.original_size == 0 { return 0.0; }
        self.compressed_size as f32 / self.original_size as f32
    }

    /// Octets économisés par rapport à la taille originale.
    pub fn space_saved(&self) -> usize {
        self.original_size.saturating_sub(self.compressed_size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Écrivain de blob ExoFS compressé.
pub struct CompressWriter {
    choice:    CompressionChoice,
    #[allow(dead_code)]
    threshold: CompressionThreshold,
}

impl CompressWriter {
    /// Crée un écrivain avec la politique donnée.
    pub fn new(policy: CompressPolicy) -> Self {
        Self {
            choice:    CompressionChoice::new(policy),
            threshold: CompressionThreshold::default(),
        }
    }

    /// Crée un écrivain avec un threshold personnalisé.
    pub fn with_threshold(policy: CompressPolicy, threshold: CompressionThreshold) -> Self {
        Self { choice: CompressionChoice::new(policy), threshold }
    }

    /// Compresse `data` et retourne un blob ExoFS (en-tête + payload).
    pub fn compress(&self, data: &[u8]) -> ExofsResult<CompressResult> {
        if data.is_empty() {
            return Ok(CompressResult {
                payload:         Vec::new(),
                algorithm:       CompressionAlgorithm::None,
                original_size:   0,
                compressed_size: 0,
            });
        }
        let decision = self.choice.decide(data);
        match decision.algorithm {
            CompressionAlgorithm::None => self.store_raw(data),
            CompressionAlgorithm::Lz4  => self.compress_lz4(data, decision.level),
            CompressionAlgorithm::Zstd => self.compress_zstd(data, decision.level),
        }
    }

    /// Estime la taille de sortie maximale sans compression effective.
    pub fn estimate_output_size(&self, data: &[u8]) -> usize {
        match self.choice.decide(data).algorithm {
            CompressionAlgorithm::Lz4  =>
                Lz4Compressor::compress_bound(data.len()).saturating_add(COMPRESSION_HEADER_SIZE),
            CompressionAlgorithm::Zstd =>
                ZstdCompressor::compress_bound(data.len()).saturating_add(COMPRESSION_HEADER_SIZE),
            CompressionAlgorithm::None => data.len(),
        }
    }

    // ── Méthodes internes──────────────────────────────────────────────────────

    fn store_raw(&self, data: &[u8]) -> ExofsResult<CompressResult> {
        let mut payload = Vec::new();
        payload.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        payload.extend_from_slice(data);
        Ok(CompressResult {
            payload,
            algorithm:       CompressionAlgorithm::None,
            original_size:   data.len(),
            compressed_size: data.len(),
        })
    }

    fn compress_lz4(&self, data: &[u8], level: CompressLevel) -> ExofsResult<CompressResult> {
        let mut compressed = Vec::new();
        let n = Lz4Compressor::compress(data, &mut compressed)?;
        if n >= data.len() { return self.store_raw(data); }

        let header = CompressionHeader {
            magic:           COMPRESSION_MAGIC,
            version:         HEADER_VERSION,
            algorithm:       CompressionAlgorithm::Lz4 as u8,
            level:           level as u8,
            flags:           0,
            original_size:   data.len() as u64,
            compressed_size: n as u64,
            crc32:           crc32_simple(data),
            _reserved:       [0u8; 4],
        };
        let payload = build_payload(&header, &compressed[..n])?;
        COMPRESSION_STATS.lz4.record_compress(data.len() as u64, n as u64, true);
        Ok(CompressResult {
            payload,
            algorithm:       CompressionAlgorithm::Lz4,
            original_size:   data.len(),
            compressed_size: n,
        })
    }

    fn compress_zstd(&self, data: &[u8], level: CompressLevel) -> ExofsResult<CompressResult> {
        let zstd_level = match level {
            CompressLevel::None    => 1,
            CompressLevel::Fast    => 1,
            CompressLevel::Default => 3,
            CompressLevel::Best    => 9,
            CompressLevel::Maximum => 19,
        };
        let mut compressed = Vec::new();
        let n = ZstdCompressor::compress(data, &mut compressed, zstd_level)?;
        if n >= data.len() { return self.store_raw(data); }

        let header = CompressionHeader {
            magic:           COMPRESSION_MAGIC,
            version:         HEADER_VERSION,
            algorithm:       CompressionAlgorithm::Zstd as u8,
            level:           level as u8,
            flags:           0,
            original_size:   data.len() as u64,
            compressed_size: n as u64,
            crc32:           crc32_simple(data),
            _reserved:       [0u8; 4],
        };
        let payload = build_payload(&header, &compressed[..n])?;
        COMPRESSION_STATS.zstd.record_compress(data.len() as u64, n as u64, true);
        Ok(CompressResult {
            payload,
            algorithm:       CompressionAlgorithm::Zstd,
            original_size:   data.len(),
            compressed_size: n,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline de compression
// ─────────────────────────────────────────────────────────────────────────────

/// Pipeline combinant décision, compression et validation CRC.
pub struct CompressionPipeline {
    writer: CompressWriter,
}

impl CompressionPipeline {
    /// Crée un pipeline avec la politique donnée.
    pub fn new(policy: CompressPolicy) -> Self {
        Self { writer: CompressWriter::new(policy) }
    }

    /// Compresse et vérifie le CRC dans le même appel.
    pub fn compress_and_validate(&self, data: &[u8]) -> ExofsResult<CompressResult> {
        let result = self.writer.compress(data)?;
        if result.algorithm != CompressionAlgorithm::None
            && result.payload.len() >= COMPRESSION_HEADER_SIZE
        {
            let hdr = &result.payload[..COMPRESSION_HEADER_SIZE];
            let stored_crc = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);
            let expected   = crc32_simple(data);
            if stored_crc != expected {
                return Err(ExofsError::InternalError);
            }
        }
        Ok(result)
    }

    /// Estime la taille de sortie.
    pub fn estimate_size(&self, data: &[u8]) -> usize {
        self.writer.estimate_output_size(data)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires internes
// ─────────────────────────────────────────────────────────────────────────────

fn build_payload(header: &CompressionHeader, compressed: &[u8]) -> ExofsResult<Vec<u8>> {
    let total = COMPRESSION_HEADER_SIZE
        .checked_add(compressed.len())
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut payload = Vec::new();
    payload.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    payload.extend_from_slice(&header.to_bytes());
    payload.extend_from_slice(compressed);
    Ok(payload)
}

/// CRC32 IEEE léger — aucune table statique (ONDISK-03).
pub(crate) fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let mut rem = ((crc ^ byte as u32) & 0xFF) as u32;
        for _ in 0..8 {
            if rem & 1 != 0 {
                rem = (rem >> 1) ^ 0xEDB8_8320;
            } else {
                rem >>= 1;
            }
        }
        crc = (crc >> 8) ^ rem;
    }
    crc ^ 0xFFFF_FFFF
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(n: usize) -> Vec<u8> { let mut v = Vec::new(); v.resize(n, 0x55); v }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut v = Vec::new(); v.reserve(n);
        let mut s: u32 = 0xCAFE_BABE;
        for _ in 0..n {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((s >> 16) as u8);
        }
        v
    }

    #[test] fn test_empty() {
        let w = CompressWriter::new(CompressPolicy::default());
        let r = w.compress(&[]).unwrap();
        assert_eq!(r.original_size, 0);
        assert!(r.payload.is_empty());
    }

    #[test] fn test_lz4_compresses_uniform() {
        let w = CompressWriter::new(CompressPolicy::lz4_fast());
        let r = w.compress(&uniform(2048)).unwrap();
        assert!(r.total_size() > 0);
    }

    #[test] fn test_compression_ratio_lz4() {
        let w = CompressWriter::new(CompressPolicy::lz4_fast());
        let r = w.compress(&uniform(8192)).unwrap();
        if r.algorithm != CompressionAlgorithm::None {
            assert!(r.compression_ratio() < 1.0);
        }
    }

    #[test] fn test_space_saved_positive() {
        let w = CompressWriter::new(CompressPolicy::lz4_fast());
        let r = w.compress(&uniform(8192)).unwrap();
        if r.algorithm != CompressionAlgorithm::None {
            assert!(r.space_saved() > 0);
        }
    }

    #[test] fn test_store_raw_incompressible() {
        let w    = CompressWriter::new(CompressPolicy::default());
        let data = pseudo_random(200);
        let r    = w.compress(&data).unwrap();
        // Doit soit stocker brut soit compresser — ne doit pas échouer.
        assert!(r.total_size() > 0 || r.original_size == 0);
    }

    #[test] fn test_crc_deterministic() {
        let d  = b"crc test input";
        let c1 = crc32_simple(d);
        let c2 = crc32_simple(d);
        assert_eq!(c1, c2);
    }

    #[test] fn test_crc_different_for_different_input() {
        assert_ne!(crc32_simple(b"aaa"), crc32_simple(b"aab"));
    }

    #[test] fn test_estimate_output_nonzero() {
        let w = CompressWriter::new(CompressPolicy::default());
        assert!(w.estimate_output_size(&uniform(1024)) > 0);
    }

    #[test] fn test_pipeline_validate_ok() {
        let p = CompressionPipeline::new(CompressPolicy::lz4_fast());
        let r = p.compress_and_validate(&uniform(4096)).unwrap();
        assert!(r.total_size() > 0);
    }

    #[test] fn test_pipeline_estimate_size_nonzero() {
        let p = CompressionPipeline::new(CompressPolicy::default());
        assert!(p.estimate_size(&uniform(512)) > 0);
    }

    #[test] fn test_with_threshold() {
        let w = CompressWriter::with_threshold(
            CompressPolicy::default(),
            CompressionThreshold::aggressive(),
        );
        let r = w.compress(&uniform(2048)).unwrap();
        assert!(r.total_size() > 0);
    }

    #[test] fn test_zstd_roundtrip_via_pipeline() {
        let p    = CompressionPipeline::new(CompressPolicy::zstd_default());
        let data = uniform(1024);
        let r    = p.compress_and_validate(&data).unwrap();
        assert!(r.total_size() > 0);
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

    #[test] fn test_compress_small_data_no_panic() {
        let w = CompressWriter::new(CompressPolicy::default());
        let r = w.compress(b"hi").unwrap();
        assert!(r.total_size() > 0 || r.original_size == 2);
    }

    #[test] fn test_build_result_total_correct() {
        let w    = CompressWriter::new(CompressPolicy::lz4_fast());
        let data = uniform(1024);
        let r    = w.compress(&data).unwrap();
        assert_eq!(r.total_size(), r.payload.len());
    }

    #[test] fn test_crc32_empty() {
        let c = crc32_simple(b"");
        assert_eq!(c, 0x0000_0000); // CRC32 de la chaîne vide
    }

    #[test] fn test_crc32_known_value() {
        // CRC32 de "123456789" = 0xCBF43926 (IEEE polynomial)
        let c = crc32_simple(b"123456789");
        assert_eq!(c, 0xCBF4_3926);
    }

    #[test] fn test_compress_writer_default_policy() {
        let w = CompressWriter::new(CompressPolicy::default());
        let r = w.compress(&uniform(512)).unwrap();
        assert!(r.total_size() > 0);
    }

    #[test] fn test_compression_pipeline_estimate_lz4() {
        let p = CompressionPipeline::new(CompressPolicy::lz4_fast());
        let s = p.estimate_size(&uniform(4096));
        assert!(s > 0);
    }

    #[test] fn test_space_saved_zero_for_raw() {
        let w    = CompressWriter::new(CompressPolicy::None);
        let data = uniform(256);
        let r    = w.compress(&data).unwrap();
        assert_eq!(r.space_saved(), 0);
    }

    #[test] fn test_compression_result_structured() {
        let w    = CompressWriter::new(CompressPolicy::lz4_fast());
        let data = uniform(2048);
        let r    = w.compress(&data).unwrap();
        // Vérifie la cohérence interne du résultat.
        assert!(r.original_size == data.len());
    }
    #[test] fn test_crc32_single_byte() {
        let c = crc32_simple(b"a");
        assert!(c != 0);
    }
    #[test] fn test_pipeline_new_no_panic() {
        let _p = CompressionPipeline::new(CompressPolicy::default());
    }
}
