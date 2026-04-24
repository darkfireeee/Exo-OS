//! Wrapper Zstd pour ExoFS — implémentation embarquée Ring 0 (Zstd frame minimal).
//!
//! L'implémentation embarquée supporte :
//!   - Raw_Literals block (Block_Type = 0)
//!   - RLE_Literals block (Block_Type = 1)
//!   Compressed blocks (Block_Type = 2) ne sont pas supportés dans le kernel ;
//!   un vrai payload compressé par Zstd doit utiliser le binding zstd-sys.
//!
//! RÈGLE OOM-02   : try_reserve avant tout push/resize.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::alloc::{alloc, dealloc, Layout};
use alloc::vec::Vec;
use core::ffi::c_void;

/// Niveau de compression par défaut (conforme zstd).
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;
/// Niveau de compression maximum (conforme zstd).
pub const ZSTD_MAX_LEVEL: i32 = 22;
/// Magic number Zstd (RFC 8878 §3.1.1).
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

/// Shim `ZSTD_malloc` pour environnements noyau `no_std`.
///
/// Signature attendue par `zstd-sys` : `(opaque, size) -> ptr`.
#[no_mangle]
pub extern "C" fn ZSTD_malloc(_opaque: *mut c_void, size: usize) -> *mut c_void {
    let layout = match zstd_alloc_layout(size) {
        Some(l) => l,
        None => return core::ptr::null_mut(),
    };
    // SAFETY: layout validé ci-dessus.
    let base = unsafe { alloc(layout) };
    if base.is_null() {
        return core::ptr::null_mut();
    }

    let header_ptr = base as *mut ZstdAllocHeader;
    // SAFETY: header_ptr pointe vers une zone allouée de taille >= header.
    unsafe {
        (*header_ptr).size = size;
    }

    let payload = base.wrapping_add(core::mem::size_of::<ZstdAllocHeader>());
    payload as *mut c_void
}

/// Shim `ZSTD_free` pour environnements noyau `no_std`.
///
/// Signature attendue par `zstd-sys` : `(opaque, address)`.
#[no_mangle]
pub extern "C" fn ZSTD_free(_opaque: *mut c_void, address: *mut c_void) {
    if address.is_null() {
        return;
    }
    let payload = address as *mut u8;
    let base = payload.wrapping_sub(core::mem::size_of::<ZstdAllocHeader>());
    let header_ptr = base as *mut ZstdAllocHeader;

    // SAFETY: base a été produit par ZSTD_malloc, header valide.
    let size = unsafe { (*header_ptr).size };
    let layout = match zstd_alloc_layout(size) {
        Some(l) => l,
        None => return,
    };

    // SAFETY: base/layout correspondent à l'allocation effectuée dans ZSTD_malloc.
    unsafe { dealloc(base, layout) };
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration du compresseur Zstd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZstdConfig {
    /// Niveau de compression 1–22 (saturé automatiquement).
    pub level: i32,
}

impl ZstdConfig {
    /// Configuration par défaut (niveau 3).
    pub const fn default_config() -> Self {
        Self {
            level: ZSTD_DEFAULT_LEVEL,
        }
    }

    /// Configuration haute qualité (niveau 9).
    pub const fn high_quality() -> Self {
        Self { level: 9 }
    }

    /// Niveau saturé entre 1 et ZSTD_MAX_LEVEL.
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
// Compresseur principal
// ─────────────────────────────────────────────────────────────────────────────

/// Compresseur/décompresseur Zstd (bloc stateless).
pub struct ZstdCompressor;

impl ZstdCompressor {
    /// Compresse `input` vers `output` avec le niveau spécifié.
    ///
    /// OOM-02 : try_reserve avant resize.
    pub fn compress(input: &[u8], output: &mut Vec<u8>, level: i32) -> ExofsResult<usize> {
        let _ = level; // niveau ignoré dans l'impl minimaliste (tout = raw)
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

    /// Décompresse `input` vers `output` (taille décompressée connue).
    ///
    /// OOM-02 : try_reserve avant resize.
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

    /// Compresse et retourne un nouveau `Vec<u8>`.
    pub fn compress_to_vec(input: &[u8], level: i32) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::compress(input, &mut out, level)?;
        Ok(out)
    }

    /// Décompresse et retourne un nouveau `Vec<u8>`.
    pub fn decompress_to_vec(input: &[u8], decompressed_size: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::decompress(input, &mut out, decompressed_size)?;
        Ok(out)
    }

    /// Retourne la borne supérieure du buffer de sortie pour `n` octets.
    pub fn compress_bound(n: usize) -> usize {
        zstd_compress_bound(n)
    }

    /// Teste si `data` commence par le magic Zstd.
    pub fn is_zstd_frame(data: &[u8]) -> bool {
        if data.len() < 4 {
            return false;
        }
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        magic == ZSTD_MAGIC
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Décodeur par étapes (streaming stateful)
// ─────────────────────────────────────────────────────────────────────────────

/// Décodeur Zstd avec état (supporte plusieurs appels sequentiels).
pub struct ZstdDecoder {
    config: ZstdConfig,
    bytes_decoded: usize,
}

impl ZstdDecoder {
    /// Crée un nouveau décodeur avec la configuration par défaut.
    pub const fn new() -> Self {
        Self {
            config: ZstdConfig::default_config(),
            bytes_decoded: 0,
        }
    }

    /// Crée un décodeur avec une configuration personnalisée.
    pub const fn with_config(config: ZstdConfig) -> Self {
        Self {
            config,
            bytes_decoded: 0,
        }
    }

    /// Décode `input` et accumule dans `output`. Retourne les octets ajoutés.
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

    /// Retourne le total d'octets décompressés depuis la création.
    pub fn total_decoded(&self) -> usize {
        self.bytes_decoded
    }

    /// Réinitialise le compteur.
    pub fn reset(&mut self) {
        self.bytes_decoded = 0;
    }

    /// Retourne la configuration.
    pub fn config(&self) -> ZstdConfig {
        self.config
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Implémentation frame minimale
// ─────────────────────────────────────────────────────────────────────────────

fn zstd_compress_bound(n: usize) -> usize {
    n.saturating_add(128).saturating_add(n >> 8)
}

/// Produit un Zstd Frame valide contenant les données en mode Raw_Literals.
fn zstd_compress_raw(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut p = 0usize;

    // Magic (4 octets).
    if p + 4 > dst.len() {
        return None;
    }
    dst[p..p + 4].copy_from_slice(&ZSTD_MAGIC.to_le_bytes());
    p += 4;

    // Frame Descriptor minimal : FHD = 0x00 (Single_Segment=0, no checksum).
    if p >= dst.len() {
        return None;
    }
    dst[p] = 0x00;
    p += 1;

    // Block Header (3 octets) : Last_Block=1, Block_Type=Raw(0b00), Block_Size.
    let bsize = src.len() as u32;
    let bh = (bsize << 3) | 0x01u32; // Last_Block bit
    if p + 3 > dst.len() {
        return None;
    }
    dst[p] = (bh & 0xFF) as u8;
    dst[p + 1] = ((bh >> 8) & 0xFF) as u8;
    dst[p + 2] = ((bh >> 16) & 0xFF) as u8;
    p += 3;

    // Données brutes.
    if p + src.len() > dst.len() {
        return None;
    }
    dst[p..p + src.len()].copy_from_slice(src);
    p += src.len();

    Some(p)
}

/// Décompresse un Zstd Frame (Raw_Literals + RLE_Literals supportés).
fn zstd_decompress_frame(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut sp = 0usize;
    let mut dp = 0usize;

    // Vérifie magic.
    if sp + 4 > src.len() {
        return None;
    }
    let magic = u32::from_le_bytes(src[sp..sp + 4].try_into().ok()?);
    if magic != ZSTD_MAGIC {
        return None;
    }
    sp += 4;

    // Frame Descriptor — on ignore, avance d'1 octet (implémentation minimale).
    if sp >= src.len() {
        return None;
    }
    sp += 1;

    // Traitement itératif des blocs.
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
                // Raw_Literals : copie directe.
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
                // RLE_Literals : un octet répété `block_size` fois.
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
            _ => return None, // Compressed_Block non supporté.
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
        let n = ZstdCompressor::compress(&[], &mut out, 3).unwrap();
        assert_eq!(n, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn test_roundtrip_uniform() {
        let input = uniform_data(512, 0x77);
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).unwrap();
        let decompressed = ZstdCompressor::decompress_to_vec(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_roundtrip_random() {
        let input = pseudo_random(256);
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).unwrap();
        let decompressed = ZstdCompressor::decompress_to_vec(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_roundtrip_text() {
        let text = b"Zstd Test Frame ExoFS kernel module -- Ring 0 no_std Rust.";
        let compressed = ZstdCompressor::compress_to_vec(text, 9).unwrap();
        let decompressed = ZstdCompressor::decompress_to_vec(&compressed, text.len()).unwrap();
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
        let comp = ZstdCompressor::compress_to_vec(&input, 3).unwrap();
        let mut dec = ZstdDecoder::new();
        let mut out = Vec::new();
        dec.decode(&comp, &mut out).unwrap();
        assert_eq!(dec.total_decoded(), input.len());
    }

    #[test]
    fn test_decoder_reset() {
        let input = uniform_data(100, 0x10);
        let comp = ZstdCompressor::compress_to_vec(&input, 3).unwrap();
        let mut dec = ZstdDecoder::new();
        let mut out = Vec::new();
        dec.decode(&comp, &mut out).unwrap();
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
        let compressed = ZstdCompressor::compress_to_vec(&input, 3).unwrap();
        let decompressed = ZstdCompressor::decompress_to_vec(&compressed, input.len()).unwrap();
        assert_eq!(decompressed.len(), input.len());
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

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
        let comp = ZstdCompressor::compress_to_vec(&data, 3).unwrap();
        let dec = ZstdCompressor::decompress_to_vec(&comp, data.len()).unwrap();
        assert_eq!(dec, data);
    }
}
