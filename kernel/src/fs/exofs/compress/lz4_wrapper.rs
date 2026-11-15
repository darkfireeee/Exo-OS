//! LZ4 wrapper for ExoFS — via lz4_flex crate (block mode, no_std).
//!
//! RULE CRYPTO-CRATES : NEVER implement from scratch.
//! Crate: lz4_flex v0.11.x, default-features = false
//!   - Pure Rust LZ4 block format implementation, no_std + alloc
//!   - Conforms to official LZ4 block format (lz4.org)
//!   - No magic number / frame header — raw compressed (block mode)
//!
//! RULE OOM-02   : try_reserve before any push.
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE RECUR-01 : no recursion — all loops are inside lz4_flex.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

use lz4_flex::block::{compress_into, decompress_into, get_maximum_output_size};

/// LZ4 compressor (block mode) — lz4_flex crate wrapper.
pub struct Lz4Compressor;

impl Lz4Compressor {
    /// Compresses `input` and appends result to `output`.
    /// Returns compressed size added.
    ///
    /// OOM-02: try_reserve before any resize.
    pub fn compress(input: &[u8], output: &mut Vec<u8>) -> ExofsResult<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let bound = get_maximum_output_size(input.len());
        let prev_len = output.len();
        output
            .try_reserve(bound)
            .map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + bound, 0u8);

        let n =
            compress_into(input, &mut output[prev_len..]).map_err(|_| ExofsError::InternalError)?;

        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Decompresses `input` into `output` (known decompressed size).
    ///
    /// OOM-02: try_reserve before any resize.
    pub fn decompress(
        input: &[u8],
        output: &mut Vec<u8>,
        decompressed_size: usize,
    ) -> ExofsResult<usize> {
        let prev_len = output.len();
        output
            .try_reserve(decompressed_size)
            .map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + decompressed_size, 0u8);

        let n = decompress_into(input, &mut output[prev_len..])
            .map_err(|_| ExofsError::CorruptedStructure)?;

        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Returns upper bound of output buffer for an input of size `n`.
    #[inline]
    pub fn compress_bound(n: usize) -> usize {
        get_maximum_output_size(n)
    }

    /// Compresses and returns a new Vec.
    ///
    /// OOM-02: try_reserve via Self::compress.
    pub fn compress_to_vec(input: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::compress(input, &mut out)?;
        Ok(out)
    }

    /// Decompresses and returns a new Vec.
    ///
    /// OOM-02: try_reserve via Self::decompress.
    pub fn decompress_to_vec(input: &[u8], decompressed_size: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::decompress(input, &mut out, decompressed_size)?;
        Ok(out)
    }
}
