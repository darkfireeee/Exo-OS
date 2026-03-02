//! Écrivain de blob compressé ExoFS.
//!
//! RÈGLE 11 : BlobId = Blake3(données AVANT compression) — calculé AVANT d'appeler ce writer.
//! RÈGLE 2  : try_reserve avant tout push.

use alloc::vec::Vec;
use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::compress::compress_choice::{CompressDecision, CompressionChoice, CompressPolicy};
use crate::fs::exofs::compress::compress_header::CompressionHeader;
use crate::fs::exofs::compress::compress_stats::COMPRESSION_STATS;
use crate::fs::exofs::compress::compress_threshold::CompressionThreshold;
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::FsError;

/// Résultat d'une compression de blob.
pub struct CompressResult {
    /// Données finales à stocker (en-tête + données compressées ou brutes).
    pub payload: Vec<u8>,
    /// Algorithme utilisé.
    pub algorithm: CompressionAlgorithm,
    /// Taille originale.
    pub original_size: usize,
    /// Taille compressée (hors en-tête).
    pub compressed_size: usize,
}

/// Écrivain de blob compressé.
pub struct CompressWriter {
    choice: CompressionChoice,
    threshold: CompressionThreshold,
}

impl CompressWriter {
    pub fn new(policy: CompressPolicy) -> Self {
        Self {
            choice: CompressionChoice::new(policy),
            threshold: CompressionThreshold::default(),
        }
    }

    /// Compresse `data` et retourne un `CompressResult`.
    ///
    /// Si la compression n'est pas bénéfique, stocke les données brutes avec
    /// un en-tête `CompressionAlgorithm::None`.
    pub fn compress(&self, data: &[u8]) -> Result<CompressResult, FsError> {
        let decision = self.choice.decide(data);

        let (algo, compressed) = match decision.algorithm {
            CompressionAlgorithm::None => (CompressionAlgorithm::None, None),
            CompressionAlgorithm::Lz4 => {
                let mut out = Vec::new();
                match Lz4Compressor::compress(data, &mut out) {
                    Ok(_) if self.threshold.is_worth_storing(out.len(), data.len()) => {
                        COMPRESSION_STATS.lz4.record_compress(data.len() as u64, out.len() as u64, true);
                        (CompressionAlgorithm::Lz4, Some(out))
                    }
                    _ => {
                        COMPRESSION_STATS.record_skip();
                        (CompressionAlgorithm::None, None)
                    }
                }
            }
            CompressionAlgorithm::Zstd => {
                let mut out = Vec::new();
                let level = decision.level as i32;
                match ZstdCompressor::compress(data, &mut out, level) {
                    Ok(_) if self.threshold.is_worth_storing(out.len(), data.len()) => {
                        COMPRESSION_STATS.zstd.record_compress(data.len() as u64, out.len() as u64, true);
                        (CompressionAlgorithm::Zstd, Some(out))
                    }
                    _ => {
                        COMPRESSION_STATS.record_skip();
                        (CompressionAlgorithm::None, None)
                    }
                }
            }
        };

        let (payload_data, compressed_size) = match compressed {
            Some(c) => {
                let cs = c.len();
                (c, cs)
            },
            None => {
                let mut raw = Vec::new();
                raw.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
                raw.extend_from_slice(data);
                let len = raw.len();
                (raw, len)
            }
        };

        // CRC32 des données compressées.
        let crc = crc32_simple(&payload_data);
        let header = CompressionHeader::new(
            algo,
            decision.level as u8,
            data.len() as u64,
            compressed_size as u64,
            crc,
        );
        let header_bytes = header.to_bytes();

        let mut payload = Vec::new();
        payload
            .try_reserve(header_bytes.len() + payload_data.len())
            .map_err(|_| FsError::OutOfMemory)?;
        payload.extend_from_slice(&header_bytes);
        payload.extend_from_slice(&payload_data);

        Ok(CompressResult {
            payload,
            algorithm: algo,
            original_size: data.len(),
            compressed_size,
        })
    }
}

/// CRC32 simple (polynomial IEEE 0x04C11DB7 reflected).
fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}
