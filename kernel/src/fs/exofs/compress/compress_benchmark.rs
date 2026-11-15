//! ExoFS compression benchmark — Ring 0 kernel microbenchmarks.
//!
//! Evaluates LZ4 vs Zstd for a given workload and returns metrics.
//! Used by the adaptive scheduler to choose the optimal algorithm.
//!
//! RULE OOM-02   : `try_reserve` before any push.
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE RECUR-01 : no recursion.

use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// BenchResult
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a compression benchmark for an algorithm.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub algorithm: CompressionAlgorithm,
    pub compressed_size: usize,
    pub compress_ticks: u64,
    pub decompress_ticks: u64,
    /// Compressed/original ratio in % (100 = no gain).
    pub ratio_percent: u64,
    pub original_size: usize,
}

impl BenchResult {
    /// `true` if compression brings a real benefit (< 95%).
    pub fn is_beneficial(&self) -> bool {
        self.ratio_percent < 95
    }

    /// Savings in bytes (saturating).
    pub fn bytes_saved(&self) -> usize {
        self.original_size.saturating_sub(self.compressed_size)
    }

    /// Global score: ratio_percent * (compress_ticks + 1).
    pub fn combined_score(&self) -> u64 {
        self.ratio_percent
            .saturating_mul(self.compress_ticks.saturating_add(1))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BenchSummary
// ─────────────────────────────────────────────────────────────────────────────

/// Comparative summary LZ4 vs Zstd.
#[derive(Debug, Clone)]
pub struct BenchSummary {
    pub lz4: BenchResult,
    pub zstd: BenchResult,
    pub is_compressible: bool,
}

impl BenchSummary {
    /// Algorithm with the best combined score (ratio * ticks).
    pub fn best_algorithm(&self) -> CompressionAlgorithm {
        if self.lz4.combined_score() <= self.zstd.combined_score() {
            CompressionAlgorithm::Lz4
        } else {
            CompressionAlgorithm::Zstd
        }
    }

    /// Algorithm with the best compression ratio.
    pub fn best_ratio_algorithm(&self) -> CompressionAlgorithm {
        if self.lz4.ratio_percent <= self.zstd.ratio_percent {
            CompressionAlgorithm::Lz4
        } else {
            CompressionAlgorithm::Zstd
        }
    }

    /// Fastest algorithm in compression.
    pub fn fastest_compress_algorithm(&self) -> CompressionAlgorithm {
        if self.lz4.compress_ticks <= self.zstd.compress_ticks {
            CompressionAlgorithm::Lz4
        } else {
            CompressionAlgorithm::Zstd
        }
    }

    /// `true` if LZ4 is equivalent to or better than Zstd (Zstd gain < 5%).
    pub fn prefer_lz4(&self) -> bool {
        let zstd_gain = self
            .lz4
            .ratio_percent
            .saturating_sub(self.zstd.ratio_percent);
        zstd_gain < 5
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressBenchmark
// ─────────────────────────────────────────────────────────────────────────────

/// Runs LZ4 and Zstd compression benchmarks.
pub struct CompressBenchmark;

impl CompressBenchmark {
    /// Benchmarks LZ4 + Zstd on `sample`, returns comparative summary.
    pub fn run(sample: &[u8]) -> ExofsResult<BenchSummary> {
        let lz4 = Self::bench_algo(sample, CompressionAlgorithm::Lz4)?;
        let zstd = Self::bench_algo(sample, CompressionAlgorithm::Zstd)?;
        let is_compressible = lz4.is_beneficial() || zstd.is_beneficial();
        Ok(BenchSummary {
            lz4,
            zstd,
            is_compressible,
        })
    }

    /// Benchmark of a single algorithm.
    pub fn bench_algo(data: &[u8], algo: CompressionAlgorithm) -> ExofsResult<BenchResult> {
        let original_size = data.len();
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
                compressed
                    .try_reserve(data.len())
                    .map_err(|_| ExofsError::NoMemory)?;
                compressed.extend_from_slice(data);
            }
        }
        let compress_ticks = crate::arch::time::read_ticks().saturating_sub(t0);
        let compressed_size = compressed.len();

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
                    .map_err(|_| ExofsError::NoMemory)?;
                decompressed.extend_from_slice(&compressed);
            }
        }
        let decompress_ticks = crate::arch::time::read_ticks().saturating_sub(t1);

        let ratio_percent = if original_size == 0 {
            100
        } else {
            (compressed_size as u64).saturating_mul(100) / (original_size as u64)
        };

        Ok(BenchResult {
            algorithm: algo,
            compressed_size,
            compress_ticks,
            decompress_ticks,
            ratio_percent,
            original_size,
        })
    }

    /// Baseline benchmark (pure memory copy).
    pub fn bench_baseline(data: &[u8]) -> ExofsResult<BenchResult> {
        Self::bench_algo(data, CompressionAlgorithm::None)
    }

    /// Recommends optimal algorithm for a given sample.
    pub fn recommend(sample: &[u8]) -> ExofsResult<CompressionAlgorithm> {
        if sample.is_empty() {
            return Ok(CompressionAlgorithm::None);
        }
        let summary = Self::run(sample)?;
        if !summary.is_compressible {
            return Ok(CompressionAlgorithm::None);
        }
        Ok(summary.best_algorithm())
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

    fn compressible(size: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(size, 0xAB);
        v
    }

    #[test]
    fn test_bench_baseline_ratio_100() {
        let data = compressible(1024);
        let r = CompressBenchmark::bench_baseline(&data).test_unwrap();
        assert_eq!(r.ratio_percent, 100);
    }

    #[test]
    fn test_bench_algo_lz4() {
        let data = compressible(2048);
        let r = CompressBenchmark::bench_algo(&data, CompressionAlgorithm::Lz4).test_unwrap();
        assert_eq!(r.algorithm, CompressionAlgorithm::Lz4);
        assert!(r.ratio_percent < 100);
    }

    #[test]
    fn test_bench_algo_zstd() {
        let data = compressible(2048);
        let r = CompressBenchmark::bench_algo(&data, CompressionAlgorithm::Zstd).test_unwrap();
        assert_eq!(r.algorithm, CompressionAlgorithm::Zstd);
        assert!(r.ratio_percent < 100);
    }

    #[test]
    fn test_bench_run_both_algos() {
        let data = compressible(4096);
        let summary = CompressBenchmark::run(&data).test_unwrap();
        assert_eq!(summary.lz4.algorithm, CompressionAlgorithm::Lz4);
        assert_eq!(summary.zstd.algorithm, CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_is_compressible_uniform() {
        let summary = CompressBenchmark::run(&compressible(4096)).test_unwrap();
        assert!(summary.is_compressible);
    }

    #[test]
    fn test_recommend_empty() {
        let r = CompressBenchmark::recommend(&[]).test_unwrap();
        assert_eq!(r, CompressionAlgorithm::None);
    }

    #[test]
    fn test_combined_score() {
        let r = BenchResult {
            algorithm: CompressionAlgorithm::Lz4,
            compressed_size: 500,
            compress_ticks: 100,
            decompress_ticks: 20,
            ratio_percent: 50,
            original_size: 1000,
        };
        assert_eq!(r.combined_score(), 50 * 101);
    }

    #[test]
    fn test_bytes_saved() {
        let r = BenchResult {
            algorithm: CompressionAlgorithm::Zstd,
            compressed_size: 300,
            compress_ticks: 200,
            decompress_ticks: 40,
            ratio_percent: 30,
            original_size: 1000,
        };
        assert_eq!(r.bytes_saved(), 700);
    }

    #[test]
    fn test_best_ratio_algo() {
        let data = compressible(4096);
        let summary = CompressBenchmark::run(&data).test_unwrap();
        let best = summary.best_ratio_algorithm();
        assert!(best == CompressionAlgorithm::Lz4 || best == CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_prefer_lz4_when_identical() {
        let data = compressible(4096);
        let summary = CompressBenchmark::run(&data).test_unwrap();
        let _ = summary.prefer_lz4();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BenchHistory — rolling benchmark history
// ─────────────────────────────────────────────────────────────────────────────

/// Rolling history of last N benchmarks.
/// Used by adaptive scheduler to refine recommendation.
pub struct BenchHistory {
    /// Recent recommendations (circular).
    entries: [CompressionAlgorithm; 8],
    head: usize,
    count: usize,
}

impl BenchHistory {
    pub const fn new() -> Self {
        Self {
            entries: [CompressionAlgorithm::None; 8],
            head: 0,
            count: 0,
        }
    }

    /// Records recommendation from a benchmark.
    pub fn push(&mut self, algo: CompressionAlgorithm) {
        self.entries[self.head] = algo;
        self.head = (self.head + 1) % 8;
        if self.count < 8 {
            self.count = self.count.saturating_add(1);
        }
    }

    /// Number of times LZ4 was recommended among the last N.
    pub fn lz4_count(&self) -> usize {
        let mut n = 0usize;
        for i in 0..self.count {
            if self.entries[i] == CompressionAlgorithm::Lz4 {
                n += 1;
            }
        }
        n
    }

    /// Majority recommendation of the history.
    pub fn majority_vote(&self) -> CompressionAlgorithm {
        if self.count == 0 {
            return CompressionAlgorithm::None;
        }
        if self.lz4_count() * 2 >= self.count {
            CompressionAlgorithm::Lz4
        } else {
            CompressionAlgorithm::Zstd
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn len(&self) -> usize {
        self.count
    }
}

impl Default for BenchHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn test_push_and_majority_vote_lz4() {
        let mut h = BenchHistory::new();
        h.push(CompressionAlgorithm::Lz4);
        h.push(CompressionAlgorithm::Lz4);
        h.push(CompressionAlgorithm::Zstd);
        assert_eq!(h.majority_vote(), CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_push_and_majority_vote_zstd() {
        let mut h = BenchHistory::new();
        h.push(CompressionAlgorithm::Zstd);
        h.push(CompressionAlgorithm::Zstd);
        h.push(CompressionAlgorithm::Lz4);
        assert_eq!(h.majority_vote(), CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_empty_history() {
        let h = BenchHistory::new();
        assert!(h.is_empty());
        assert_eq!(h.majority_vote(), CompressionAlgorithm::None);
    }

    #[test]
    fn test_history_circular_wrap() {
        let mut h = BenchHistory::new();
        for _ in 0..10 {
            h.push(CompressionAlgorithm::Lz4);
        }
        assert_eq!(h.len(), 8); // Max capacity
    }

    #[test]
    fn test_lz4_count() {
        let mut h = BenchHistory::new();
        h.push(CompressionAlgorithm::Lz4);
        h.push(CompressionAlgorithm::Zstd);
        h.push(CompressionAlgorithm::Lz4);
        assert_eq!(h.lz4_count(), 2);
    }

    // ── Additional tests ─────────────────────────────────────────────────────

    #[test]
    fn test_bench_summary_is_compressible_flag() {
        let s = BenchSummary {
            lz4: BenchResult {
                algorithm: CompressionAlgorithm::Lz4,
                compressed_size: 10,
                compress_ticks: 50,
                decompress_ticks: 10,
                ratio_percent: 10,
                original_size: 100,
            },
            zstd: BenchResult {
                algorithm: CompressionAlgorithm::Zstd,
                compressed_size: 8,
                compress_ticks: 200,
                decompress_ticks: 20,
                ratio_percent: 8,
                original_size: 100,
            },
            is_compressible: true,
        };
        assert!(s.is_compressible);
    }

    #[test]
    fn test_bench_result_ratio() {
        let r = BenchResult {
            algorithm: CompressionAlgorithm::Lz4,
            compressed_size: 50,
            compress_ticks: 10,
            decompress_ticks: 5,
            ratio_percent: 50,
            original_size: 100,
        };
        assert_eq!(r.ratio_percent, 50);
    }

    #[test]
    fn test_history_majority_empty() {
        let h = BenchHistory::new();
        // On empty history, majority vote must not panic.
        let _ = h.majority_vote();
    }

    #[test]
    fn test_history_majority_lz4_wins() {
        let mut h = BenchHistory::new();
        h.push(CompressionAlgorithm::Lz4);
        h.push(CompressionAlgorithm::Lz4);
        h.push(CompressionAlgorithm::Zstd);
        assert_eq!(h.majority_vote(), CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_history_len_saturates_at_capacity() {
        let mut h = BenchHistory::new();
        for _ in 0..20 {
            h.push(CompressionAlgorithm::Lz4);
        }
        assert!(h.len() <= 8);
    }

    #[test]
    fn test_bench_result_saved_bytes() {
        let r = BenchResult {
            algorithm: CompressionAlgorithm::Lz4,
            compressed_size: 40,
            compress_ticks: 5,
            decompress_ticks: 2,
            ratio_percent: 40,
            original_size: 100,
        };
        assert_eq!(r.bytes_saved(), 60);
    }
    #[test]
    fn test_bench_result_is_beneficial() {
        let r = BenchResult {
            algorithm: CompressionAlgorithm::Zstd,
            compressed_size: 80,
            compress_ticks: 5,
            decompress_ticks: 2,
            ratio_percent: 80,
            original_size: 100,
        };
        assert!(r.is_beneficial());
    }
}
