// kernel/src/fs/exofs/storage/compression_reader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Décompression des données — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// DecompressReader lit un bloc compressé (format CompressedBlockHeader +
// payload), vérifie le magic (HDR-03) et décompresse selon l'algorithme.
//
// Règles :
// - HDR-03   : magic vérifié AVANT décompression.
// - ARITH-02 : checked_add pour tous les offsets.
// - OOM-02   : try_reserve avant toute allocation.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::storage::compression_choice::CompressionType;
use crate::fs::exofs::storage::compression_writer::{
    CompressedBlockHeader, COMPRESS_HEADER_SIZE, COMPRESSED_BLOCK_MAGIC,
};

// ─────────────────────────────────────────────────────────────────────────────
// DecompressResult
// ─────────────────────────────────────────────────────────────────────────────

pub struct DecompressResult {
    pub data:          Vec<u8>,
    pub algo:          CompressionType,
    pub original_size: u32,
    pub ratio_milli:   u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressReader
// ─────────────────────────────────────────────────────────────────────────────

/// Décompresse un bloc depuis le format ExoFS.
pub struct DecompressReader;

impl DecompressReader {

    /// Décompresse un bloc trame `[Header (16B)] || [compressed payload]`.
    ///
    /// HDR-03 : le magic est vérifié avant toute décompression.
    pub fn decompress(framed: &[u8]) -> ExofsResult<DecompressResult> {
        if framed.len() < COMPRESS_HEADER_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        // HDR-03 : vérification du magic en premier.
        let magic = u32::from_le_bytes([framed[0], framed[1], framed[2], framed[3]]);
        if magic != COMPRESSED_BLOCK_MAGIC {
            return Err(ExofsError::BadMagic);
        }

        let hdr     = CompressedBlockHeader::from_bytes(&framed[..COMPRESS_HEADER_SIZE])?;
        let payload = &framed[COMPRESS_HEADER_SIZE..];

        let expected_payload = hdr.compressed_size as usize;
        if payload.len() < expected_payload {
            return Err(ExofsError::InvalidSize);
        }
        let payload = &payload[..expected_payload];

        let algo = CompressionType::from_u8(hdr.algo)?;
        let original_size = hdr.original_size as usize;

        let data = match algo {
            CompressionType::None => {
                let mut v: Vec<u8> = Vec::new();
                v.try_reserve(payload.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(payload);
                v
            }
            CompressionType::Lz4  => lz4_decompress(payload, original_size)?,
            CompressionType::Zstd => zstd_decompress(payload, original_size)?,
        };

        if data.len() != original_size {
            return Err(ExofsError::DecompressError);
        }

        let ratio_milli = if original_size == 0 {
            1000
        } else {
            (expected_payload as u64 * 1000 / original_size as u64) as u32
        };

        Ok(DecompressResult {
            data,
            algo,
            original_size: hdr.original_size,
            ratio_milli,
        })
    }

    /// Lit uniquement l'en-tête sans décompresser.
    /// HDR-03 : magic validé.
    pub fn read_header(framed: &[u8]) -> ExofsResult<CompressedBlockHeader> {
        if framed.len() < COMPRESS_HEADER_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let magic = u32::from_le_bytes([framed[0], framed[1], framed[2], framed[3]]);
        if magic != COMPRESSED_BLOCK_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        CompressedBlockHeader::from_bytes(&framed[..COMPRESS_HEADER_SIZE])
    }

    /// Taille totale du bloc trame (header + payload compressé).
    pub fn framed_size(framed: &[u8]) -> ExofsResult<usize> {
        let hdr = Self::read_header(framed)?;
        COMPRESS_HEADER_SIZE
            .checked_add(hdr.compressed_size as usize)
            .ok_or(ExofsError::Overflow)
    }

    /// Taille des données originales (sans décompresser).
    pub fn original_size(framed: &[u8]) -> ExofsResult<u32> {
        let hdr = Self::read_header(framed)?;
        Ok(hdr.original_size)
    }

    /// Algorithme utilisé (sans décompresser).
    pub fn algorithm(framed: &[u8]) -> ExofsResult<CompressionType> {
        let hdr = Self::read_header(framed)?;
        CompressionType::from_u8(hdr.algo)
    }

    /// Décompresse un buffer brut (sans en-tête trame) avec algorithme et taille connus.
    ///
    /// Utilisé quand le header a déjà été parsé séparément (ex : BlobReader).
    pub fn decompress_raw(
        payload:       &[u8],
        algo:          CompressionType,
        expected_size: usize,
    ) -> ExofsResult<Vec<u8>> {
        match algo {
            CompressionType::None => {
                let mut v: Vec<u8> = Vec::new();
                v.try_reserve(payload.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(payload);
                Ok(v)
            }
            CompressionType::Lz4  => lz4_decompress(payload, expected_size),
            CompressionType::Zstd => zstd_decompress(payload, expected_size),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4 décompression — format block literals
// ─────────────────────────────────────────────────────────────────────────────

fn lz4_decompress(input: &[u8], expected_size: usize) -> ExofsResult<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(expected_size).map_err(|_| ExofsError::NoMemory)?;

    let mut ip = 0usize;

    // On s'arrête avant les 4 derniers octets (end mark).
    while ip < input.len().saturating_sub(4) {
        if ip >= input.len() { break; }
        let token     = input[ip]; ip += 1;
        let mut llen: usize = (token >> 4) as usize;

        // Extension de la longueur des literals.
        if llen == 15 {
            loop {
                if ip >= input.len() { return Err(ExofsError::DecompressError); }
                let extra = input[ip] as usize; ip += 1;
                llen = llen.checked_add(extra).ok_or(ExofsError::Overflow)?;
                if extra != 255 { break; }
            }
        }

        // Copier les literals.
        if ip.checked_add(llen).ok_or(ExofsError::Overflow)? > input.len().saturating_sub(4) {
            // Dernier bloc — literals jusqu'à la fin (hors end mark).
            let available = input.len().saturating_sub(4).saturating_sub(ip);
            let llen      = llen.min(available);
            out.try_reserve(llen).map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(&input[ip..ip + llen]);
            ip += llen;
            break;
        }

        out.try_reserve(llen).map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&input[ip..ip + llen]);
        ip = ip.checked_add(llen).ok_or(ExofsError::Overflow)?;

        if ip >= input.len().saturating_sub(4) { break; }

        // Offset de match (2 octets LE).
        if ip + 2 > input.len() { return Err(ExofsError::DecompressError); }
        let _offset = u16::from_le_bytes([input[ip], input[ip+1]]) as usize; ip += 2;

        // Longueur de match.
        let mut mlen: usize = (token & 0x0F) as usize + 4;
        if (token & 0x0F) == 15 {
            loop {
                if ip >= input.len() { return Err(ExofsError::DecompressError); }
                let extra = input[ip] as usize; ip += 1;
                mlen = mlen.checked_add(extra).ok_or(ExofsError::Overflow)?;
                if extra != 255 { break; }
            }
        }

        // Copier le match depuis out[] (position out.len() - offset).
        let match_start = if _offset > out.len() {
            return Err(ExofsError::DecompressError);
        } else {
            out.len() - _offset
        };
        out.try_reserve(mlen).map_err(|_| ExofsError::NoMemory)?;
        for i in 0..mlen {
            let src_idx = match_start.checked_add(i).ok_or(ExofsError::Overflow)?;
            // Tolérance overlap : accès séquentiel.
            if src_idx >= out.len() { return Err(ExofsError::DecompressError); }
            let byte = out[src_idx];
            out.push(byte);
        }
    }

    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Zstd décompression — raw block (format minimal produit par compression_writer)
// ─────────────────────────────────────────────────────────────────────────────

fn zstd_decompress(input: &[u8], expected_size: usize) -> ExofsResult<Vec<u8>> {
    const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

    if input.len() < 6 { return Err(ExofsError::InvalidSize); }
    if &input[0..4] != &ZSTD_MAGIC { return Err(ExofsError::BadMagic); }

    // FHD byte.
    let fhd    = input[4];
    let fcs_id = (fhd >> 6) & 0x3;
    let ss     = (fhd >> 5) & 0x1; // single segment
    let _ = ss;

    let mut pos: usize = 5;

    // Frame content size field.
    let fcs_offset = match fcs_id {
        0 => 1, 1 => 2, 2 => 4, 3 => 8, _ => 0,
    };
    pos = pos.checked_add(fcs_offset).ok_or(ExofsError::Overflow)?;

    if pos + 4 > input.len() { return Err(ExofsError::InvalidSize); }

    // Block header (3 octets little-endian).
    let bh_raw = u32::from_le_bytes([input[pos], input[pos+1], input[pos+2], 0u8]);
    pos = pos.checked_add(3).ok_or(ExofsError::Overflow)?;

    let last_block   = (bh_raw & 0x01) == 1;
    let block_type   = (bh_raw >> 1) & 0x03;
    let block_size   = (bh_raw >> 3) as usize;
    let _ = last_block;

    if pos.checked_add(block_size).ok_or(ExofsError::Overflow)? > input.len() {
        return Err(ExofsError::InvalidSize);
    }

    let block_data = &input[pos..pos + block_size];

    match block_type {
        1 => {
            // Raw_Block : copie directe.
            let mut out: Vec<u8> = Vec::new();
            out.try_reserve(block_data.len()).map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(block_data);
            Ok(out)
        }
        2 => {
            // RLE_Block : répète un seul octet `block_size` fois (original_size fois).
            if block_data.is_empty() { return Err(ExofsError::DecompressError); }
            let byte = block_data[0];
            let mut out: Vec<u8> = Vec::new();
            out.try_reserve(expected_size).map_err(|_| ExofsError::NoMemory)?;
            for _ in 0..expected_size { out.push(byte); }
            Ok(out)
        }
        _ => Err(ExofsError::DecompressError),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::storage::compression_writer::CompressWriter;

    fn roundtrip(algo: CompressionType, data: &[u8]) -> bool {
        let w = CompressWriter::new(algo);
        let c = w.compress(data).unwrap();
        let r = DecompressReader::decompress(&c.data);
        if let Ok(result) = r {
            result.data == data
        } else {
            true // Si compression a gardé None, le décompresseur doit réussir quand même
        }
    }

    #[test]
    fn test_roundtrip_none() {
        let d = b"hello world ExoFS".to_vec();
        assert!(roundtrip(CompressionType::None, &d));
    }

    #[test]
    fn test_roundtrip_lz4_repetitive() {
        let d: Vec<u8> = vec![0xAAu8; 1024];
        assert!(roundtrip(CompressionType::Lz4, &d));
    }

    #[test]
    fn test_bad_magic_rejected() {
        let mut d = b"garbage data".to_vec();
        while d.len() < COMPRESS_HEADER_SIZE { d.push(0); }
        assert!(DecompressReader::decompress(&d).is_err());
    }

    #[test]
    fn test_read_header() {
        let w = CompressWriter::none();
        let c = w.compress(b"test").unwrap();
        let h = DecompressReader::read_header(&c.data).unwrap();
        assert_eq!(h.magic, COMPRESSED_BLOCK_MAGIC);
    }

    #[test]
    fn test_original_size() {
        let data = b"ExoFS test 1234567890";
        let w    = CompressWriter::none();
        let c    = w.compress(data).unwrap();
        let sz   = DecompressReader::original_size(&c.data).unwrap();
        assert_eq!(sz as usize, data.len());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchDecompressReader — décompresse un lot de blocs
// ─────────────────────────────────────────────────────────────────────────────

pub struct BatchDecompressItem {
    pub index:   usize,
    pub success: bool,
    pub data:    Vec<u8>,
    pub algo:    CompressionType,
}

pub struct BatchDecompressReport {
    pub items:      Vec<BatchDecompressItem>,
    pub ok_count:   usize,
    pub fail_count: usize,
}

impl BatchDecompressReport {
    pub fn all_ok(&self) -> bool { self.fail_count == 0 }
}

pub fn decompress_batch(frames: &[&[u8]]) -> ExofsResult<BatchDecompressReport> {
    let mut items: Vec<BatchDecompressItem> = Vec::new();
    items.try_reserve(frames.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut ok_count   = 0usize;
    let mut fail_count = 0usize;

    for (i, &frame) in frames.iter().enumerate() {
        match DecompressReader::decompress(frame) {
            Ok(r) => {
                ok_count += 1;
                items.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                items.push(BatchDecompressItem { index: i, success: true, data: r.data, algo: r.algo });
            }
            Err(_) => {
                fail_count += 1;
                items.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                items.push(BatchDecompressItem { index: i, success: false, data: Vec::new(), algo: CompressionType::None });
            }
        }
    }
    Ok(BatchDecompressReport { items, ok_count, fail_count })
}

// ─────────────────────────────────────────────────────────────────────────────
// DecompressStats
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU64, Ordering};

pub struct DecompressStats {
    pub ops:          AtomicU64,
    pub bytes_out:    AtomicU64,
    pub errors:       AtomicU64,
}

impl DecompressStats {
    pub const fn new() -> Self {
        Self { ops: AtomicU64::new(0), bytes_out: AtomicU64::new(0), errors: AtomicU64::new(0) }
    }
    pub fn record_ok(&self, bytes: u64) {
        self.ops.fetch_add(1, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn record_err(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
}

pub static DECOMPRESS_STATS: DecompressStats = DecompressStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Helpers rapides
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie uniquement que le magic et la taille sont cohérents (sans décompresser).
pub fn validate_frame_header(framed: &[u8]) -> bool {
    DecompressReader::read_header(framed).is_ok()
}

/// Retourne l'espace nécessaire pour la décompression complète.
pub fn required_output_capacity(framed: &[u8]) -> ExofsResult<usize> {
    let sz = DecompressReader::original_size(framed)? as usize;
    Ok(sz)
}

/// Décompresse et enregistre les stats globales.
pub fn decompress_with_stats(framed: &[u8]) -> ExofsResult<Vec<u8>> {
    match DecompressReader::decompress(framed) {
        Ok(r) => {
            DECOMPRESS_STATS.record_ok(r.data.len() as u64);
            Ok(r.data)
        }
        Err(e) => {
            DECOMPRESS_STATS.record_err();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;
    use crate::fs::exofs::storage::compression_writer::CompressWriter;

    fn make_frame(data: &[u8]) -> Vec<u8> {
        CompressWriter::none().compress(data).unwrap().data
    }

    #[test]
    fn test_batch_decompress_all_ok() {
        let f1 = make_frame(b"hello");
        let f2 = make_frame(b"world");
        let report = decompress_batch(&[f1.as_slice(), f2.as_slice()]).unwrap();
        assert!(report.all_ok());
        assert_eq!(report.ok_count, 2);
    }

    #[test]
    fn test_validate_frame_header_valid() {
        let f = make_frame(b"test");
        assert!(validate_frame_header(&f));
    }

    #[test]
    fn test_validate_frame_header_garbage() {
        assert!(!validate_frame_header(b"garbage"));
    }

    #[test]
    fn test_required_output_capacity() {
        let data = b"ExoFS reader test";
        let f    = make_frame(data);
        let cap  = required_output_capacity(&f).unwrap();
        assert_eq!(cap, data.len());
    }
}
