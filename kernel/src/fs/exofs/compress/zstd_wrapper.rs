//! Zstd wrapper for ExoFS — Ring 0 embedded implementation (minimal Zstd frame).
//!
//! The embedded implementation supports:
//!   - Raw_Literals block (Block_Type = 0)
//!   - RLE_Literals block (Block_Type = 1)
//!   Compressed blocks (Block_Type = 2) are not supported in the kernel;
//!   a real Zstd compressed payload must use the zstd-sys binding.
//!
//! RULE OOM-02   : try_reserve before any push/resize.
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE RECUR-01 : no recursion.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::alloc::{alloc, dealloc, Layout};
use alloc::vec::Vec;
use core::ffi::c_void;

/// Default compression level (zstd compliant).
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;
/// Maximum compression level (zstd compliant).
pub const ZSTD_MAX_LEVEL: i32 = 22;
/// Zstd magic number (RFC 8878 §3.1.1).
const ZSTD_MAGIC: u32 = 0xFD2FB528;

#[repr(C)]
struct ZstdAllocHeader {
    size: usize,
}

#[inline]
fn zstd_alloc_layout(size: usize) -> Option<Layout> {
    let hdr = core::mem::size_of::<ZstdAllocHeader>();
    let total = hdr.checked_add(size)?;
    Layout::from_size_align(total, core::mem::align_of::<usize>()).ok()
}

/// `ZSTD_malloc` shim for `no_std` kernel environments.
///
/// Signature expected by `zstd-sys`: `(opaque, size) -> ptr`.
#[no_mangle]
pub extern "C" fn ZSTD_malloc(_opaque: *mut c_void, size: usize) -> *mut c_void {
    let layout = match zstd_alloc_layout(size) {
        Some(l) => l,
        None => return core::ptr::null_mut(),
    };
    // SAFETY: layout validated above.
    let base = unsafe { alloc(layout) };
    if base.is_null() {
        return core::ptr::null_mut();
    }

    let header_ptr = base as *mut ZstdAllocHeader;
    // SAFETY: header_ptr points to allocated zone of size >= header.
    unsafe {
        (*header_ptr).size = size;
    }

    let payload = base.wrapping_add(core::mem::size_of::<ZstdAllocHeader>());
    payload as *mut c_void
}

/// `ZSTD_free` shim for `no_std` kernel environments.
///
/// Signature expected by `zstd-sys`: `(opaque, address)`.
#[no_mangle]
pub extern "C" fn ZSTD_free(_opaque: *mut c_void, address: *mut c_void) {
    if address.is_null() {
        return;
    }
    let payload = address as *mut u8;
    let base = payload.wrapping_sub(core::mem::size_of::<ZstdAllocHeader>());
    let header_ptr = base as *mut ZstdAllocHeader;

    // SAFETY: base was produced by ZSTD_malloc, header is valid.
    let size = unsafe { (*header_ptr).size };
    let layout = match zstd_alloc_layout(size) {
        Some(l) => l,
        None => return,
    };

    // SAFETY: base/layout match allocation made in ZSTD_malloc.
    unsafe { dealloc(base, layout) };
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Zstd compressor configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZstdConfig {
    /// Compression level 1–22 (automatically clamped).
    pub level: i32,
}

impl ZstdConfig {
    /// Default configuration (level 3).
    pub const fn default_config() -> Self {
        Self {
            level: ZSTD_DEFAULT_LEVEL,
        }
    }

    /// High quality configuration (level 9).
    pub const fn high_quality() -> Self {
        Self { level: 9 }
    }

    /// Clamped level between 1 and ZSTD_MAX_LEVEL.
    pub fn clamped_level(self) -> i32 {
        self.level.max(1).min(ZSTD_MAX_LEVEL)
    }
}

impl Default for ZstdConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main compressor
// ─────────────────────────────────────────────────────────────────────────────

/// Zstd compressor/decompressor (stateless block).
pub struct ZstdCompressor;

impl ZstdCompressor {
    /// Compresses `input` into `output` with specified level.
    ///
    /// OOM-02: try_reserve before resize.
    pub fn compress(input: &[u8], output: &mut Vec<u8>, level: i32) -> ExofsResult<usize> {
        let _ = level; // level ignored in minimal implementation (all = raw)
        if input.is_empty() {
            return Ok(0);
        }
        let bound = zstd_compress_bound(input.len());
        let prev_len = output.len();
        output
            .try_reserve(bound)
            .map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + bound, 0u8);
        let n =
            zstd_compress_raw(input, &mut output[prev_len..]).ok_or(ExofsError::InternalError)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Decompresses `input` into `output` (known decompressed size).
    ///
    /// OOM-02: try_reserve before resize.
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
        let n = zstd_decompress_frame(input, &mut output[prev_len..])
            .ok_or(ExofsError::CorruptedStructure)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Compresses and returns a new `Vec<u8>`.
    pub fn compress_to_vec(input: &[u8], level: i32) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::compress(input, &mut out, level)?;
        Ok(out)
    }

    /// Decompresses and returns a new `Vec<u8>`.
    pub fn decompress_to_vec(input: &[u8], decompressed_size: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::decompress(input, &mut out, decompressed_size)?;
        Ok(out)
    }

    /// Returns upper bound of output buffer for `n` bytes.
    pub fn compress_bound(n: usize) -> usize {
        zstd_compress_bound(n)
    }

    /// Tests if `data` starts with Zstd magic.
    pub fn is_zstd_frame(data: &[u8]) -> bool {
        if data.len() < 4 {
            return false;
        }
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        magic == ZSTD_MAGIC
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stepwise decoder (stateful streaming)
// ─────────────────────────────────────────────────────────────────────────────

/// Stateful Zstd decoder (supports multiple sequential calls).
pub struct ZstdDecoder {
    config: ZstdConfig,
    bytes_decoded: usize,
}

impl ZstdDecoder {
    /// Creates a new decoder with default configuration.
    pub const fn new() -> Self {
        Self {
            config: ZstdConfig::default_config(),
            bytes_decoded: 0,
        }
    }

    /// Creates a decoder with custom configuration.
    pub const fn with_config(config: ZstdConfig) -> Self {
        Self {
            config,
            bytes_decoded: 0,
        }
    }

    /// Decodes `input` and accumulates into `output`. Returns bytes added.
    pub fn decode(&mut self, input: &[u8], output: &mut Vec<u8>) -> ExofsResult<usize> {
        let prev_len = output.len();
        let bound = input.len().saturating_add(256);
        output
            .try_reserve(bound)
            .map_err(|_| ExofsError::NoMemory)?;
        let start = output.len();
        output.resize(start + bound, 0u8);
        let n = zstd_decompress_frame(input, &mut output[start..])
            .ok_or(ExofsError::CorruptedStructure)?;
        output.truncate(start + n);
        self.bytes_decoded = self.bytes_decoded.saturating_add(n);
        Ok(output.len() - prev_len)
    }

    /// Returns total bytes decompressed since creation.
    pub fn total_decoded(&self) -> usize {
        self.bytes_decoded
    }

    /// Resets counter.
    pub fn reset(&mut self) {
        self.bytes_decoded = 0;
    }

    /// Returns configuration.
    pub fn config(&self) -> ZstdConfig {
        self.config
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Minimal frame implementation
// ─────────────────────────────────────────────────────────────────────────────

fn zstd_compress_bound(n: usize) -> usize {
    n.saturating_add(128).saturating_add(n >> 8)
}

/// Produces a valid Zstd Frame containing data in Raw_Literals mode.
fn zstd_compress_raw(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut p = 0usize;
    let use_rle = src
        .first()
        .copied()
        .map(|first| src.iter().all(|&b| b == first))
        .unwrap_or(false);

    // Magic (4 bytes).
    if p + 4 > dst.len() {
        return None;
    }
    dst[p..p + 4].copy_from_slice(&ZSTD_MAGIC.to_le_bytes());
    p += 4;

    // Minimal Frame Descriptor: FHD = 0x00 (Single_Segment=0, no checksum).
    if p >= dst.len() {
        return None;
    }
    dst[p] = 0x00;
    p += 1;

    // Block Header (3 bytes): Last_Block=1, Block_Type=Raw(0b00) or RLE(0b01), Block_Size.
    let bsize = src.len() as u32;
    let block_type = if use_rle { 0x02u32 } else { 0x00u32 };
    let bh = (bsize << 3) | block_type | 0x01u32; // Last_Block bit
    if p + 3 > dst.len() {
        return None;
    }
    dst[p] = (bh & 0xFF) as u8;
    dst[p + 1] = ((bh >> 8) & 0xFF) as u8;
    dst[p + 2] = ((bh >> 16) & 0xFF) as u8;
    p += 3;

    if use_rle {
        if p >= dst.len() {
            return None;
        }
        dst[p] = src[0];
        p += 1;
    } else {
        // Raw data.
        if p + src.len() > dst.len() {
            return None;
        }
        dst[p..p + src.len()].copy_from_slice(src);
        p += src.len();
    }

    Some(p)
}

/// Decompresses a Zstd Frame (Raw_Literals + RLE_Literals supported).
fn zstd_decompress_frame(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut sp = 0usize;
    let mut dp = 0usize;

    // Check magic.
    if sp + 4 > src.len() {
        return None;
    }
    let magic = u32::from_le_bytes(src[sp..sp + 4].try_into().ok()?);
    if magic != ZSTD_MAGIC {
        return None;
    }
    sp += 4;

    // Frame Descriptor — ignored, advance 1 byte (minimal implementation).
    if sp >= src.len() {
        return None;
    }
    sp += 1;

    // Iterative block processing.
    loop {
        if sp + 3 > src.len() {
            return None;
        }
        let b0 = src[sp] as u32;
        let b1 = src[sp + 1] as u32;
        let b2 = src[sp + 2] as u32;
        let bh = b0 | (b1 << 8) | (b2 << 16);
        sp += 3;

        let last_block = (bh & 0x01) != 0;
        let block_type = (bh >> 1) & 0x03;
        let block_size = (bh >> 3) as usize;

        match block_type {
            0 => {
                // Raw_Literals: direct copy.
                if sp + block_size > src.len() {
                    return None;
                }
                if dp + block_size > dst.len() {
                    return None;
                }
                dst[dp..dp + block_size].copy_from_slice(&src[sp..sp + block_size]);
                sp += block_size;
                dp += block_size;
            }
            1 => {
                // RLE_Literals: single byte repeated `block_size` times.
                if sp >= src.len() {
                    return None;
                }
                let byte = src[sp];
                sp += 1;
                if dp + block_size > dst.len() {
                    return None;
                }
                for i in 0..block_size {
                    dst[dp + i] = byte;
                }
                dp += block_size;
            }
            _ => return None, // Compressed_Block not supported.
        }

        if last_block {
            break;
        }
    }

    Some(dp)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_data(n: usize, byte: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.resize(n, byte);
        v
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.reserve(n);
        let mut s: u32 = 0xABCD_1234;
        for _ in 0..n {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            v.push((s >> 16) as u8);
        }
        v
    }

    #[test]
    fn test_is_zstd_frame_empty() {
        assert!(!ZstdCompressor::is_zstd_frame(&[]));
    }

    #[test]
    fn test_is_zstd_frame_short() {
        assert!(!ZstdCompressor::is_zstd_frame(&[0xFD, 0x2F]));
    }

    #[test]
    fn test_is_zstd_frame_valid() {
        let data = ZSTD_MAGIC.to_le_bytes();
        assert!(ZstdCompressor::is_zstd_frame(&data));
    }

    #[test]
    fn test_compress_empty() {
        let mut out = Vec::new();
        let n = ZstdCompressor::compress(&[], &mut out, 3).test_unwrap();
        assert_eq!(n, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn test_roundtrip_uniform() {
        let input = uniform_data(512, 0x77);
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).test_unwrap();
        let decompressed =
            ZstdCompressor::decompress_to_vec(&compressed, input.len()).test_unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_roundtrip_random() {
        let input = pseudo_random(256);
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).test_unwrap();
        let decompressed =
            ZstdCompressor::decompress_to_vec(&compressed, input.len()).test_unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_roundtrip_text() {
        let text = b"Zstd Test Frame ExoFS kernel module -- Ring 0 no_std Rust.";
        let compressed = ZstdCompressor::compress_to_vec(text, 9).test_unwrap();
        let decompressed = ZstdCompressor::decompress_to_vec(&compressed, text.len()).test_unwrap();
        assert_eq!(decompressed.as_slice(), text);
    }

    #[test]
    fn test_compress_bound_positive() {
        assert!(ZstdCompressor::compress_bound(1000) > 1000);
    }

    #[test]
    fn test_config_clamp() {
        let c = ZstdConfig { level: 100 };
        assert_eq!(c.clamped_level(), ZSTD_MAX_LEVEL);
        let c2 = ZstdConfig { level: -5 };
        assert_eq!(c2.clamped_level(), 1);
    }

    #[test]
    fn test_decoder_total_decoded() {
        let input = uniform_data(200, 0xAB);
        let comp = ZstdCompressor::compress_to_vec(&input, 3).test_unwrap();
        let mut dec = ZstdDecoder::new();
        let mut out = Vec::new();
        dec.decode(&comp, &mut out).test_unwrap();
        assert_eq!(dec.total_decoded(), input.len());
    }

    #[test]
    fn test_decoder_reset() {
        let input = uniform_data(100, 0x10);
        let comp = ZstdCompressor::compress_to_vec(&input, 3).test_unwrap();
        let mut dec = ZstdDecoder::new();
        let mut out = Vec::new();
        dec.decode(&comp, &mut out).test_unwrap();
        dec.reset();
        assert_eq!(dec.total_decoded(), 0);
    }

    #[test]
    fn test_decompress_bad_magic() {
        let bad = [0x00u8, 0x01, 0x02, 0x03, 0x04];
        let mut out = Vec::new();
        let r = ZstdCompressor::decompress(&bad, &mut out, 100);
        assert!(r.is_err());
    }

    #[test]
    fn test_roundtrip_large() {
        let input = uniform_data(8192, 0x55);
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).test_unwrap();
        let decompressed =
            ZstdCompressor::decompress_to_vec(&compressed, input.len()).test_unwrap();
        assert_eq!(decompressed.len(), input.len());
    }

    // ── Additional tests ─────────────────────────────────────────────────────

    #[test]
    fn test_config_high_quality_level() {
        let c = ZstdConfig::high_quality();
        assert_eq!(c.level, 9);
    }

    #[test]
    fn test_decoder_with_config() {
        let c = ZstdConfig { level: 6 };
        let d = ZstdDecoder::with_config(c);
        assert_eq!(d.config().level, 6);
    }

    #[test]
    fn test_roundtrip_binary() {
        let data: Vec<u8> = (0u8..=255).collect();
        let comp = ZstdCompressor::compress_to_vec(&data, 3).test_unwrap();
        let dec = ZstdCompressor::decompress_to_vec(&comp, data.len()).test_unwrap();
        assert_eq!(dec, data);
    }
}
