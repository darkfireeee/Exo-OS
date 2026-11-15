//! CdcChunker — Content-Defined Chunking with Rabin-Karp rolling hash (no_std).
//!
//! Implements CDC with a Rabin-Karp sliding window to find chunk boundaries
//! independent of stream positioning.
//!
//! RULE: boundaries are deterministic for the same data stream.
//!
//! RECUR-01: no recursion — while loop.
//! OOM-02  : try_reserve on all Vecs.
//! ARITH-02: checked / saturating / wrapping + mulmod61 for Rabin.

use super::chunk_fingerprint::{blake3_hash, fnv1a64};
use super::chunking::{
    ChunkBoundary, ChunkStats, Chunker, DedupChunk, CHUNK_MAX_PER_BLOB, CHUNK_MAX_SIZE,
    CHUNK_MIN_SIZE,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// CDC Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const CDC_MIN_SIZE: usize = 2048; // 2 KiB minimum.
pub const CDC_AVG_SIZE: usize = 8192; // 8 KiB target.
pub const CDC_MAX_SIZE: usize = 65536; // 64 KiB maximum.
/// Mask triggering a cut (log2(avg_size) bits to 0).
#[allow(dead_code)]
const CDC_MASK: u64 = (CDC_AVG_SIZE as u64) - 1; // = 0x1FFF
/// Base of Rabin polynomial.
const RABIN_BASE: u64 = 257;
/// Mersenne prime modulus M61 = 2^61 - 1.
const RABIN_MOD: u64 = (1u64 << 61) - 1;
/// Sliding window size.
const WINDOW_SIZE: usize = 64;
/// RABIN_BASE^WINDOW_SIZE mod M61 (computed at compile time).
const RABIN_POW: u64 = pow_mod(RABIN_BASE, WINDOW_SIZE as u64, RABIN_MOD);

// ─────────────────────────────────────────────────────────────────────────────
// M61 Arithmetic
// ─────────────────────────────────────────────────────────────────────────────

/// M61 modular multiplication without 128-bit overflow.
///
/// ARITH-02: intermediate u128 to prevent 64-bit overflow.
const fn mulmod61(a: u64, b: u64) -> u64 {
    let p = (a as u128) * (b as u128);
    let hi = (p >> 61) as u64;
    let lo = (p & RABIN_MOD as u128) as u64;
    let r = hi + lo;
    if r >= RABIN_MOD {
        r - RABIN_MOD
    } else {
        r
    }
}

/// M61 modular exponentiation (RECUR-01: loop — no recursion).
const fn pow_mod(mut base: u64, mut exp: u64, modulus: u64) -> u64 {
    let mut result = 1u64;
    base %= modulus;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mulmod61(result, base);
        }
        exp >>= 1;
        base = mulmod61(base, base);
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
// RollingHash — Rabin-Karp sliding window
// ─────────────────────────────────────────────────────────────────────────────

/// Rabin-Karp sliding window for CDC.
struct RollingHash {
    window: [u8; WINDOW_SIZE],
    wpos: usize,
    hash: u64,
}

impl RollingHash {
    fn new() -> Self {
        Self {
            window: [0u8; WINDOW_SIZE],
            wpos: 0,
            hash: 0,
        }
    }

    /// Advances the window by one byte and updates the hash.
    ///
    /// ARITH-02: wrapping_add / wrapping_sub via mulmod61.
    fn roll(&mut self, byte: u8) -> u64 {
        let old = self.window[self.wpos] as u64;
        self.window[self.wpos] = byte;
        self.wpos = (self.wpos + 1) % WINDOW_SIZE;
        // hash = hash * base - old * base^W + new
        let term_old = mulmod61(old, RABIN_POW);
        let term_new = byte as u64 % RABIN_MOD;
        let h = mulmod61(self.hash, RABIN_BASE);
        // M61 modular subtraction.
        let h = if h >= term_old {
            h - term_old
        } else {
            h + RABIN_MOD - term_old
        };
        let h = h + term_new;
        self.hash = if h >= RABIN_MOD { h - RABIN_MOD } else { h };
        self.hash
    }

    /// Resets the window to zero.
    #[allow(dead_code)]
    fn reset(&mut self) {
        self.window = [0u8; WINDOW_SIZE];
        self.wpos = 0;
        self.hash = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CdcConfig
// ─────────────────────────────────────────────────────────────────────────────

/// CDC chunker configuration.
#[derive(Debug, Clone, Copy)]
pub struct CdcConfig {
    pub min_size: usize,
    pub avg_size: usize,
    pub max_size: usize,
    pub include_data: bool,
}

impl CdcConfig {
    pub fn default() -> Self {
        Self {
            min_size: CDC_MIN_SIZE,
            avg_size: CDC_AVG_SIZE,
            max_size: CDC_MAX_SIZE,
            include_data: true,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.min_size < CHUNK_MIN_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        if self.max_size > CHUNK_MAX_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        if self.min_size >= self.max_size {
            return Err(ExofsError::InvalidArgument);
        }
        if self.avg_size < self.min_size || self.avg_size > self.max_size {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    /// CDC mask computed from avg_size.
    fn mask(&self) -> u64 {
        (self.avg_size as u64).saturating_sub(1)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CdcChunker
// ─────────────────────────────────────────────────────────────────────────────

/// CDC (Content-Defined Chunking) chunker using Rabin-Karp rolling hash.
pub struct CdcChunker {
    config: CdcConfig,
}

impl CdcChunker {
    pub fn new(config: CdcConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn default_chunker() -> Self {
        Self {
            config: CdcConfig::default(),
        }
    }

    pub fn with_sizes(min: usize, avg: usize, max: usize) -> ExofsResult<Self> {
        Self::new(CdcConfig {
            min_size: min,
            avg_size: avg,
            max_size: max,
            include_data: true,
        })
    }

    /// Looks for the CDC boundary within the range `data[start..]`.
    ///
    /// Returns the chunk size from `start`.
    /// RECUR-01: for loop — no recursion.
    fn find_boundary(&self, data: &[u8], start: usize) -> usize {
        let total = data.len();
        let min = self.config.min_size;
        let max = self.config.max_size;
        let mask = self.config.mask();
        let end = total - start;
        if end <= min {
            return end;
        }
        let mut rh = RollingHash::new();
        let cut_end = end.min(max);
        // Phase 1: advance to minimum without looking for a boundary.
        for i in 0..min.min(cut_end) {
            rh.roll(data[start + i]);
        }
        // Phase 2: search for CDC boundary.
        for i in min..cut_end {
            let h = rh.roll(data[start + i]);
            if h & mask == 0 {
                return i + 1;
            }
        }
        cut_end
    }
}

impl Chunker for CdcChunker {
    /// Splits `data` into CDC chunks.
    ///
    /// RECUR-01: while loop.
    /// OOM-02  : try_reserve.
    fn chunk(&self, data: &[u8]) -> ExofsResult<Vec<DedupChunk>> {
        let total = data.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let mut chunks: Vec<DedupChunk> = Vec::new();
        let mut pos: usize = 0;
        while pos < total {
            if chunks.len() >= CHUNK_MAX_PER_BLOB {
                return Err(ExofsError::OffsetOverflow);
            }
            let len = self.find_boundary(data, pos);
            let raw = &data[pos..pos.saturating_add(len)];
            let blake3 = blake3_hash(raw);
            let fast = fnv1a64(raw);
            let boundary = ChunkBoundary::new(pos as u64, len as u32)?;
            let chunk = if self.config.include_data {
                chunks.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                DedupChunk::new(boundary, blake3, fast, raw)?
            } else {
                chunks.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                DedupChunk::metadata_only(boundary, blake3, fast)
            };
            chunks.push(chunk);
            pos = pos.saturating_add(len);
        }
        Ok(chunks)
    }

    fn boundaries(&self, data: &[u8]) -> ExofsResult<Vec<ChunkBoundary>> {
        let total = data.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let mut bounds: Vec<ChunkBoundary> = Vec::new();
        let mut pos: usize = 0;
        while pos < total {
            if bounds.len() >= CHUNK_MAX_PER_BLOB {
                return Err(ExofsError::OffsetOverflow);
            }
            let len = self.find_boundary(data, pos);
            bounds.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            bounds.push(ChunkBoundary::new(pos as u64, len as u32)?);
            pos = pos.saturating_add(len);
        }
        Ok(bounds)
    }

    fn min_chunk_size(&self) -> usize {
        self.config.min_size
    }
    fn max_chunk_size(&self) -> usize {
        self.config.max_size
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CdcStats — CDC chunking statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Advanced statistics for CDC chunking.
#[derive(Debug, Clone)]
pub struct CdcStats {
    pub base: ChunkStats,
    pub forced_max_cuts: usize, // Chunks cut at max_size (no natural boundary).
    pub natural_cuts: usize,    // Chunks cut on natural CDC boundary.
}

impl CdcStats {
    /// Computes CDC stats from a list of chunks and the config.
    pub fn compute(chunks: &[DedupChunk], config: &CdcConfig) -> ExofsResult<Self> {
        let base = ChunkStats::from_chunks(chunks)?;
        let mut forced = 0usize;
        let mut natural = 0usize;
        for c in chunks {
            if c.boundary.length as usize == config.max_size {
                forced = forced.saturating_add(1);
            } else {
                natural = natural.saturating_add(1);
            }
        }
        Ok(Self {
            base,
            forced_max_cuts: forced,
            natural_cuts: natural,
        })
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

    fn cdc() -> CdcChunker {
        CdcChunker::default_chunker()
    }

    #[test]
    fn test_cdc_empty_data() {
        assert!(cdc().chunk(&[]).test_unwrap().is_empty());
    }

    #[test]
    fn test_cdc_small_data_single_chunk() {
        let data = &[0u8; CDC_MIN_SIZE - 1];
        let chunks = cdc().chunk(data).test_unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].boundary.length as usize, CDC_MIN_SIZE - 1);
    }

    #[test]
    fn test_cdc_respects_min_size() {
        let data = &[0u8; CDC_AVG_SIZE * 4];
        let cdc = cdc();
        let cs = cdc.chunk(data).test_unwrap();
        for c in &cs {
            if c.boundary.offset > 0 {
                assert!(
                    (c.boundary.length as usize) >= CDC_MIN_SIZE
                        || c.boundary.offset as usize + c.boundary.length as usize == data.len()
                );
            }
        }
    }

    #[test]
    fn test_cdc_respects_max_size() {
        let data = &[0u8; CDC_MAX_SIZE * 2];
        let cs = cdc().chunk(data).test_unwrap();
        for c in &cs {
            assert!((c.boundary.length as usize) <= CDC_MAX_SIZE);
        }
    }

    #[test]
    fn test_cdc_boundaries_cover_entire_data() {
        let data = &[0u8; CDC_AVG_SIZE * 3];
        let cdc = cdc();
        let bounds = cdc.boundaries(data).test_unwrap();
        let total: u64 = bounds.iter().map(|b| b.length as u64).sum();
        assert_eq!(total, data.len() as u64);
    }

    #[test]
    fn test_cdc_deterministic() {
        let data = &[0xABu8; CDC_AVG_SIZE * 2];
        let c1 = cdc().chunk(data).test_unwrap();
        let c2 = cdc().chunk(data).test_unwrap();
        assert_eq!(c1.len(), c2.len());
        for (a, b) in c1.iter().zip(c2.iter()) {
            assert_eq!(a.blake3, b.blake3);
            assert_eq!(a.boundary, b.boundary);
        }
    }

    #[test]
    fn test_cdc_fingerprints_differ() {
        let d1 = &[0x00u8; CDC_AVG_SIZE * 2];
        let d2 = &[0xFFu8; CDC_AVG_SIZE * 2];
        let c1 = cdc().chunk(d1).test_unwrap();
        let c2 = cdc().chunk(d2).test_unwrap();
        // At least one different chunk.
        let any_diff = c1.iter().zip(c2.iter()).any(|(a, b)| a.blake3 != b.blake3);
        assert!(any_diff);
    }

    #[test]
    fn test_cdc_invalid_config() {
        assert!(CdcChunker::with_sizes(10, 4096, 65536).is_err()); // min too small
        assert!(CdcChunker::with_sizes(2048, 100, 65536).is_err()); // avg < min
    }

    #[test]
    fn test_cdc_stats() {
        let data = &[0u8; CDC_AVG_SIZE * 3];
        let cdc = cdc();
        let chunks = cdc.chunk(data).test_unwrap();
        let stats = CdcStats::compute(&chunks, &cdc.config).test_unwrap();
        assert!(stats.base.total_chunks > 0);
    }

    #[test]
    fn test_mulmod61_no_overflow() {
        let a: u64 = RABIN_MOD - 1;
        let b: u64 = RABIN_MOD - 1;
        let r = mulmod61(a, b);
        assert!(r < RABIN_MOD);
    }

    #[test]
    fn test_pow_mod_identity() {
        assert_eq!(pow_mod(2, 0, RABIN_MOD), 1);
        assert_eq!(pow_mod(1, 100, RABIN_MOD), 1);
    }

    #[test]
    fn test_rolling_hash_rolls() {
        let mut rh = RollingHash::new();
        let h1 = rh.roll(0x41);
        let h2 = rh.roll(0x42);
        assert_ne!(h1, h2);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AdaptiveCdcChunker — adaptive CDC with multiple granularity levels
// ─────────────────────────────────────────────────────────────────────────────

/// Granularity level for adaptive CDC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcSharpness {
    Coarse, // avg = CDC_MAX_SIZE / 2
    Normal, // avg = CDC_AVG_SIZE
    Fine,   // avg = CDC_MIN_SIZE * 2
}

impl CdcSharpness {
    pub fn config(&self) -> CdcConfig {
        match self {
            CdcSharpness::Coarse => CdcConfig {
                min_size: CDC_MIN_SIZE * 4,
                avg_size: CDC_MAX_SIZE / 2,
                max_size: CDC_MAX_SIZE,
                include_data: true,
            },
            CdcSharpness::Normal => CdcConfig::default(),
            CdcSharpness::Fine => CdcConfig {
                min_size: CDC_MIN_SIZE,
                avg_size: CDC_MIN_SIZE * 2,
                max_size: CDC_AVG_SIZE,
                include_data: true,
            },
        }
    }
}

/// Adaptive CDC chunker that chooses granularity level based on size.
pub struct AdaptiveCdcChunker {
    coarse: CdcChunker,
    normal: CdcChunker,
    fine: CdcChunker,
}

impl AdaptiveCdcChunker {
    pub fn new() -> ExofsResult<Self> {
        Ok(Self {
            coarse: CdcChunker::new(CdcSharpness::Coarse.config())?,
            normal: CdcChunker::new(CdcSharpness::Normal.config())?,
            fine: CdcChunker::new(CdcSharpness::Fine.config())?,
        })
    }

    /// Chooses the chunker adapted to the blob size.
    ///
    /// RECUR-01: no recursion.
    pub fn chunk_adaptive(&self, data: &[u8]) -> ExofsResult<Vec<DedupChunk>> {
        let len = data.len();
        if len >= CDC_MAX_SIZE * 8 {
            self.coarse.chunk(data)
        } else if len >= CDC_AVG_SIZE {
            self.normal.chunk(data)
        } else {
            self.fine.chunk(data)
        }
    }
}

#[cfg(test)]
mod tests_adaptive {
    use super::*;

    #[test]
    fn test_adaptive_large() {
        let data = &[0u8; CDC_MAX_SIZE * 10];
        let adapt = AdaptiveCdcChunker::new().test_unwrap();
        let cs = adapt.chunk_adaptive(data).test_unwrap();
        assert!(!cs.is_empty());
    }

    #[test]
    fn test_adaptive_small() {
        let data = &[0x55u8; CDC_MIN_SIZE * 3];
        let adapt = AdaptiveCdcChunker::new().test_unwrap();
        let cs = adapt.chunk_adaptive(data).test_unwrap();
        assert!(!cs.is_empty());
    }

    #[test]
    fn test_sharpness_configs_valid() {
        assert!(CdcChunker::new(CdcSharpness::Coarse.config()).is_ok());
        assert!(CdcChunker::new(CdcSharpness::Normal.config()).is_ok());
        assert!(CdcChunker::new(CdcSharpness::Fine.config()).is_ok());
    }
}
