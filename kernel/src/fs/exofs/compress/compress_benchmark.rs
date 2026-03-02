//! Benchmark de compression ExoFS — microbenchmarks kernel Ring 0.
//!
//! Évalue LZ4 vs Zstd pour une charge donnée et retourne les métriques.
//! Utilisé par le scheduler adaptatif pour choisir l'algorithme optimal.

use alloc::vec::Vec;
use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::FsError;

/// Résultat d'un benchmark de compression.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub algorithm: CompressionAlgorithm,
    /// Taille compressée en bytes.
    pub compressed_size: usize,
    /// Durée de compression en ticks CPU.
    pub compress_ticks: u64,
    /// Durée de décompression en ticks CPU.
    pub decompress_ticks: u64,
    /// Ratio (0–100 : 100 = aucun bénéfice).
    pub ratio_percent: u64,
}

/// Lance un benchmark LZ4 + Zstd sur les données fournies.
pub struct CompressBenchmark;

impl CompressBenchmark {
    /// Benchmark des deux algorithmes sur `sample`.
    /// Retourne les résultats triés par `ratio_percent` croissant (meilleur en premier).
    pub fn run(sample: &[u8]) -> Result<[BenchResult; 2], FsError> {
        let lz4 = Self::bench_algo(sample, CompressionAlgorithm::Lz4)?;
        let zstd = Self::bench_algo(sample, CompressionAlgorithm::Zstd)?;
        Ok([lz4, zstd])
    }

    /// Benchmark d'un algorithme unique.
    pub fn bench_algo(
        data: &[u8],
        algo: CompressionAlgorithm,
    ) -> Result<BenchResult, FsError> {
        let mut compressed = Vec::new();

        let t0 = crate::arch::time::read_ticks();
        match algo {
            CompressionAlgorithm::Lz4 => {
                Lz4Compressor::compress(data, &mut compressed)?;
            }
            CompressionAlgorithm::Zstd => {
                ZstdCompressor::compress(data, &mut compressed, 3)?;
            }
            CompressionAlgorithm::None => {
                compressed.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
                compressed.extend_from_slice(data);
            }
        }
        let compress_ticks = crate::arch::time::read_ticks().saturating_sub(t0);

        let compressed_size = compressed.len();

        // Décompression.
        let mut decompressed = Vec::new();
        let t1 = crate::arch::time::read_ticks();
        match algo {
            CompressionAlgorithm::Lz4 => {
                Lz4Compressor::decompress(&compressed, &mut decompressed, data.len())?;
            }
            CompressionAlgorithm::Zstd => {
                ZstdCompressor::decompress(&compressed, &mut decompressed, data.len())?;
            }
            CompressionAlgorithm::None => {
                decompressed
                    .try_reserve(compressed.len())
                    .map_err(|_| FsError::OutOfMemory)?;
                decompressed.extend_from_slice(&compressed);
            }
        }
        let decompress_ticks = crate::arch::time::read_ticks().saturating_sub(t1);

        let ratio_percent = if data.is_empty() {
            100
        } else {
            (compressed_size as u64 * 100) / (data.len() as u64)
        };

        Ok(BenchResult {
            algorithm: algo,
            compressed_size,
            compress_ticks,
            decompress_ticks,
            ratio_percent,
        })
    }
}
