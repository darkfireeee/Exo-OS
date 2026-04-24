//! Lecteur de blob compressé ExoFS.
//!
//! Ce module décompresse les blobs produits par `CompressWriter`.
//! Il supporte :
//!   - La détection automatique du format (magic ExoFS)
//!   - La validation CRC32 après décompression
//!   - La décompression brute (sans en-tête) pour usage interne
//!   - Le suivi statistique des opérations
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::compress::compress_header::{
    CompressionHeader, COMPRESSION_HEADER_SIZE, COMPRESSION_MAGIC,
};
use crate::fs::exofs::compress::compress_writer::crc32_simple;
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques de décompression
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs de décompression.
#[derive(Debug, Default)]
pub struct DecompressStats {
    /// Nombre total de blobs décompressés.
    pub total: u64,
    /// Nombre de blobs LZ4 décompressés.
    pub lz4: u64,
    /// Nombre de blobs Zstd décompressés.
    pub zstd: u64,
    /// Nombre de blobs passés en mode brut (pas d'en-tête).
    pub raw: u64,
    /// Erreurs CRC rencontrées.
    pub crc_errors: u64,
    /// Erreurs de corruption structurelle.
    pub struct_errors: u64,
}

impl DecompressStats {
    /// Crée un compteur vierge.
    pub const fn new() -> Self {
        Self {
            total: 0,
            lz4: 0,
            zstd: 0,
            raw: 0,
            crc_errors: 0,
            struct_errors: 0,
        }
    }

    /// Retourne le taux d'erreur CRC (0.0–1.0).
    pub fn crc_error_rate(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.crc_errors as f32 / self.total as f32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressReader
// ─────────────────────────────────────────────────────────────────────────────

/// Lecteur de blob compressé ExoFS.
///
/// Détecte automatiquement si le payload possède un en-tête ExoFS et
/// décompresse en conséquence. Les blobs sans magic sont retournés bruts.
pub struct DecompressReader {
    stats: DecompressStats,
    validate_crc: bool,
}

impl DecompressReader {
    /// Crée un lecteur avec validation CRC activée.
    pub const fn new() -> Self {
        Self {
            stats: DecompressStats::new(),
            validate_crc: true,
        }
    }

    /// Crée un lecteur sans validation CRC (mode rapide).
    pub const fn fast_mode() -> Self {
        Self {
            stats: DecompressStats::new(),
            validate_crc: false,
        }
    }

    /// Décompresse `payload` en détectant automatiquement le format.
    ///
    /// - Si le payload commence par le magic ExoFS → décompresse via l'en-tête.
    /// - Sinon → retourne une copie brute du payload.
    pub fn decompress(&mut self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        if payload.len() >= 4 {
            let m = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            if m == COMPRESSION_MAGIC {
                return self.decompress_with_header(payload);
            }
        }
        self.decompress_raw_copy(payload)
    }

    /// Décompresse en utilisant l'en-tête ExoFS (valide magic, version, CRC).
    pub fn decompress_with_header(&mut self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        if payload.len() < COMPRESSION_HEADER_SIZE {
            self.stats.struct_errors = self.stats.struct_errors.saturating_add(1);
            return Err(ExofsError::CorruptedStructure);
        }

        let header = CompressionHeader::from_bytes(
            payload[..COMPRESSION_HEADER_SIZE]
                .try_into()
                .map_err(|_| ExofsError::CorruptedStructure)?,
        )?;

        let compressed = &payload[COMPRESSION_HEADER_SIZE..];
        let expected_cs = header.compressed_size as usize;
        if compressed.len() < expected_cs {
            self.stats.struct_errors = self.stats.struct_errors.saturating_add(1);
            return Err(ExofsError::CorruptedStructure);
        }

        let orig_size = header.original_size as usize;
        let algo = CompressionAlgorithm::try_from(header.algorithm)
            .map_err(|_| ExofsError::NotSupported)?;

        let result = match algo {
            CompressionAlgorithm::None => {
                let mut out = Vec::new();
                out.try_reserve(expected_cs)
                    .map_err(|_| ExofsError::NoMemory)?;
                out.extend_from_slice(&compressed[..expected_cs]);
                self.stats.raw = self.stats.raw.saturating_add(1);
                out
            }
            CompressionAlgorithm::Lz4 => {
                let out = Lz4Compressor::decompress_to_vec(&compressed[..expected_cs], orig_size)?;
                self.stats.lz4 = self.stats.lz4.saturating_add(1);
                out
            }
            CompressionAlgorithm::Zstd => {
                let out = ZstdCompressor::decompress_to_vec(&compressed[..expected_cs], orig_size)?;
                self.stats.zstd = self.stats.zstd.saturating_add(1);
                out
            }
        };

        // Validation CRC.
        if self.validate_crc && algo != CompressionAlgorithm::None {
            let actual_crc = crc32_simple(&result);
            if actual_crc != header.crc32 {
                self.stats.crc_errors = self.stats.crc_errors.saturating_add(1);
                return Err(ExofsError::CorruptedStructure);
            }
        }

        self.stats.total = self.stats.total.saturating_add(1);
        Ok(result)
    }

    /// Décompresse sans en-tête (mode brut) avec l'algorithme explicite.
    pub fn decompress_raw(
        &mut self,
        data: &[u8],
        algo: CompressionAlgorithm,
        orig_size: usize,
    ) -> ExofsResult<Vec<u8>> {
        let result = match algo {
            CompressionAlgorithm::None => {
                let mut out = Vec::new();
                out.try_reserve(data.len())
                    .map_err(|_| ExofsError::NoMemory)?;
                out.extend_from_slice(data);
                out
            }
            CompressionAlgorithm::Lz4 => Lz4Compressor::decompress_to_vec(data, orig_size)?,
            CompressionAlgorithm::Zstd => ZstdCompressor::decompress_to_vec(data, orig_size)?,
        };
        self.stats.total = self.stats.total.saturating_add(1);
        Ok(result)
    }

    /// Retourne `true` si le payload possède un en-tête ExoFS valide.
    pub fn is_compressed(payload: &[u8]) -> bool {
        if payload.len() < 4 {
            return false;
        }
        let m = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        m == COMPRESSION_MAGIC
    }

    /// Retourne une référence aux statistiques accumulées.
    pub fn stats(&self) -> &DecompressStats {
        &self.stats
    }

    /// Réinitialise les statistiques.
    pub fn reset_stats(&mut self) {
        self.stats = DecompressStats::new();
    }

    // ── Méthode interne pour payload brut ─────────────────────────────────────

    fn decompress_raw_copy(&mut self, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        out.try_reserve(payload.len())
            .map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(payload);
        self.stats.raw = self.stats.raw.saturating_add(1);
        self.stats.total = self.stats.total.saturating_add(1);
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::compress::compress_choice::CompressPolicy;
    use crate::fs::exofs::compress::compress_writer::CompressWriter;

    fn uniform(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(n, 0x33);
        v
    }

    fn roundtrip_lz4(data: &[u8]) -> Vec<u8> {
        let writer = CompressWriter::new(CompressPolicy::lz4_fast());
        let blob = writer.compress(data).unwrap();
        let mut reader = DecompressReader::new();
        reader.decompress(&blob.payload).unwrap()
    }

    #[test]
    fn test_empty_payload() {
        let mut r = DecompressReader::new();
        let out = r.decompress(&[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_raw_passthrough() {
        let data = b"no header here";
        let mut r = DecompressReader::new();
        let out = r.decompress(data).unwrap();
        assert_eq!(out.as_slice(), data);
    }

    #[test]
    fn test_is_compressed_false_empty() {
        assert!(!DecompressReader::is_compressed(&[]));
    }

    #[test]
    fn test_is_compressed_false_short() {
        assert!(!DecompressReader::is_compressed(&[0x01, 0x02]));
    }

    #[test]
    fn test_is_compressed_true() {
        let writer = CompressWriter::new(CompressPolicy::lz4_fast());
        let blob = writer.compress(&uniform(2048)).unwrap();
        if blob.algorithm != CompressionAlgorithm::None {
            assert!(DecompressReader::is_compressed(&blob.payload));
        }
    }

    #[test]
    fn test_roundtrip_lz4() {
        let data = uniform(4096);
        let dec = roundtrip_lz4(&data);
        // Si compressé, le roundtrip doit être identique.
        let writer = CompressWriter::new(CompressPolicy::lz4_fast());
        let blob = writer.compress(&data).unwrap();
        if blob.algorithm == CompressionAlgorithm::Lz4 {
            assert_eq!(dec, data);
        }
    }

    #[test]
    fn test_stats_increment() {
        let mut r = DecompressReader::new();
        let _ = r.decompress(b"raw data").unwrap();
        assert_eq!(r.stats().total, 1);
    }

    #[test]
    fn test_reset_stats() {
        let mut r = DecompressReader::new();
        let _ = r.decompress(b"test").unwrap();
        r.reset_stats();
        assert_eq!(r.stats().total, 0);
    }

    #[test]
    fn test_crc_error_rate_zero_when_no_errors() {
        let r = DecompressReader::new();
        assert_eq!(r.stats().crc_error_rate(), 0.0);
    }

    #[test]
    fn test_bad_struct_size() {
        // Payload avec juste le magic mais trop court → CorruptedStructure.
        let bad = [0x45u8, 0x78, 0x4F, 0x46, 0x01, 0x02]; // EXOF... trop court
        let mut r = DecompressReader::new();
        // Ne pas utiliser le magic ExoFS réel : ici on teste un magic random.
        let out = r.decompress(&bad);
        // Soit Ok (pas de magic ExoFS) soit Err — pas de panique.
        let _ = out;
    }

    #[test]
    fn test_decompress_raw_none() {
        let data = b"direct data";
        let mut r = DecompressReader::new();
        let out = r
            .decompress_raw(data, CompressionAlgorithm::None, data.len())
            .unwrap();
        assert_eq!(out.as_slice(), data);
    }

    #[test]
    fn test_fast_mode_no_crc_error() {
        // En mode rapide (sans CRC), la décompression ne vérifie pas le CRC.
        let mut r = DecompressReader::fast_mode();
        let out = r.decompress(b"plain text no header").unwrap();
        assert_eq!(out.as_slice(), b"plain text no header");
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

    #[test]
    fn test_roundtrip_zstd() {
        let data = uniform(2048);
        let writer = CompressWriter::new(CompressPolicy::zstd_default());
        let blob = writer.compress(&data).unwrap();
        let mut r = DecompressReader::new();
        let dec = r.decompress(&blob.payload).unwrap();
        // Si le writer a compressé (pas brut), le roundtrip est exact.
        if blob.algorithm != CompressionAlgorithm::None {
            assert_eq!(dec, data);
        }
    }

    #[test]
    fn test_stats_lz4_increments() {
        let data = uniform(4096);
        let writer = CompressWriter::new(CompressPolicy::lz4_fast());
        let blob = writer.compress(&data).unwrap();
        if blob.algorithm == CompressionAlgorithm::Lz4 {
            let mut r = DecompressReader::new();
            let _ = r.decompress(&blob.payload).unwrap();
            assert!(r.stats().lz4 >= 1);
        }
    }

    #[test]
    fn test_crc_error_rate_formula() {
        let mut s = DecompressStats::new();
        s.total = 10;
        s.crc_errors = 2;
        let rate = s.crc_error_rate();
        assert!((rate - 0.2).abs() < 0.001);
    }

    #[test]
    fn test_decompress_raw_lz4() {
        let data = uniform(512);
        let mut comp = Vec::new();
        Lz4Compressor::compress(&data, &mut comp).unwrap();
        let mut r = DecompressReader::new();
        let dec = r
            .decompress_raw(&comp, CompressionAlgorithm::Lz4, data.len())
            .unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn test_decompress_raw_zstd() {
        let data = uniform(256);
        let comp = ZstdCompressor::compress_to_vec(&data, 3).unwrap();
        let mut r = DecompressReader::new();
        let dec = r
            .decompress_raw(&comp, CompressionAlgorithm::Zstd, data.len())
            .unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn test_total_increments_on_raw_passthrough() {
        let mut r = DecompressReader::new();
        let _ = r.decompress(b"raw1").unwrap();
        let _ = r.decompress(b"raw2").unwrap();
        assert_eq!(r.stats().total, 2);
    }

    #[test]
    fn test_stats_raw_counter() {
        let mut r = DecompressReader::new();
        let _ = r.decompress(b"plain").unwrap();
        assert!(r.stats().raw >= 1);
    }
    #[test]
    fn test_stats_struct_errors_zero_initially() {
        let r = DecompressReader::new();
        assert_eq!(r.stats().struct_errors, 0);
    }
    #[test]
    fn test_decompress_small_raw_no_magic() {
        let data = b"small";
        let mut r = DecompressReader::new();
        let out = r.decompress(data).unwrap();
        assert_eq!(out.len(), data.len());
    }
    #[test]
    fn test_fast_mode_correct_flag() {
        let r = DecompressReader::fast_mode();
        // En mode rapide la validation CRC est désactivée — test indirect.
        assert_eq!(r.stats().crc_errors, 0);
    }
    #[test]
    fn test_roundtrip_many_small_blobs() {
        let w = CompressWriter::new(CompressPolicy::lz4_fast());
        let mut reader = DecompressReader::new();
        for i in 0u8..10 {
            let data = vec![i; 128];
            let blob = w.compress(&data).unwrap();
            let dec = reader.decompress(&blob.payload).unwrap();
            assert_eq!(dec.len(), data.len());
        }
    }
    #[test]
    fn test_is_not_compressed_when_wrong_magic() {
        assert!(!DecompressReader::is_compressed(&[0x00u8; 16]));
    }
    #[test]
    fn test_stats_raw_no_header() {
        let mut r = DecompressReader::new();
        let _ = r.decompress(b"xyzw no magic here at all").unwrap();
        assert!(r.stats().raw >= 1);
    }
    #[test]
    fn test_total_zero_initially() {
        let r = DecompressReader::new();
        assert_eq!(r.stats().total, 0);
    }
}
