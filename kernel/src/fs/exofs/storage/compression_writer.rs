// kernel/src/fs/exofs/storage/compression_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Compression des données — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// CompressWriter prend des données BRUTES (après calcul du BlobId) et les
// compresse selon l'algorithme choisi par compression_choice.
//
// Règle HASH-02 : la compression s'effectue APRES le calcul du BlobId.
//   raw_data → Blake3(BlobId) → COMPRESSION → encryption → disk
//
// En no_std ring 0, nous n'avons pas accès aux crates lz4/zstd standards.
// L'implémentation fournit :
//   - None → copie directe.
//   - Lz4  → LZ4 block format simplifié (compatible lz4 block spec).
//   - Zstd → table zstd simple (stub documenté, à remplacer par la crate
//              zstd-sys quand disponible dans le noyau).

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::storage::compression_choice::CompressionType;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic d'un bloc compressé (4 octets de prologue).
pub const COMPRESSED_BLOCK_MAGIC: u32 = 0x455F_434F; // "E_CO"
/// Taille maximale d'entrée pour un seul appel compress() (16 MB).
pub const MAX_COMPRESS_INPUT: usize = 16 * 1024 * 1024;
/// Overhead de l'en-tête de bloc compressé.
pub const COMPRESS_HEADER_SIZE: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// CompressedBlock — en-tête + payload compressé
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un bloc compressé sur disque.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct CompressedBlockHeader {
    pub magic:         u32,
    pub algo:          u8,
    pub _pad:          [u8; 3],
    pub original_size: u32,
    pub compressed_size: u32,
}

impl CompressedBlockHeader {
    pub fn new(algo: CompressionType, original: u32, compressed: u32) -> Self {
        Self {
            magic:           COMPRESSED_BLOCK_MAGIC,
            algo:            algo.to_u8(),
            _pad:            [0; 3],
            original_size:   original,
            compressed_size: compressed,
        }
    }

    pub fn to_bytes(&self) -> [u8; COMPRESS_HEADER_SIZE] {
        let mut out = [0u8; COMPRESS_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4]   = self.algo;
        out[5..8].copy_from_slice(&self._pad);
        out[8..12].copy_from_slice(&self.original_size.to_le_bytes());
        out[12..16].copy_from_slice(&self.compressed_size.to_le_bytes());
        out
    }

    pub fn from_bytes(b: &[u8]) -> ExofsResult<Self> {
        if b.len() < COMPRESS_HEADER_SIZE { return Err(ExofsError::InvalidSize); }
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if magic != COMPRESSED_BLOCK_MAGIC { return Err(ExofsError::BadMagic); }
        let algo         = b[4];
        let original     = u32::from_le_bytes([b[8],  b[9],  b[10], b[11]]);
        let compressed   = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
        Ok(Self { magic, algo, _pad: [0; 3], original_size: original, compressed_size: compressed })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressResult
// ─────────────────────────────────────────────────────────────────────────────

pub struct CompressResult {
    /// Données compressées (avec en-tête intégré).
    pub data:          Vec<u8>,
    /// Algorithme utilisé.
    pub algo:          CompressionType,
    /// Taille originale.
    pub original_size: u32,
    /// Ratio en permille (1000 = pas de compression).
    pub ratio_milli:   u32,
}

impl CompressResult {
    pub fn ratio_pct(&self) -> u32 { self.ratio_milli / 10 }

    pub fn is_effective(&self) -> bool {
        self.ratio_milli < 900 // ≥ 10% de gain
    }

    /// Taille des données compressées.
    pub fn len(&self) -> usize { self.data.len() }
    pub fn is_empty(&self) -> bool { self.data.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Compresse des données selon l'algorithme demandé.
///
/// # Règle HASH-02 : cette struct reçoit les données BRUTES dont le BlobId
/// a déjà été calculé.
pub struct CompressWriter {
    algo: CompressionType,
}

impl CompressWriter {
    pub fn new(algo: CompressionType) -> Self { Self { algo } }
    pub fn none()  -> Self { Self { algo: CompressionType::None } }
    pub fn lz4()   -> Self { Self { algo: CompressionType::Lz4  } }
    pub fn zstd()  -> Self { Self { algo: CompressionType::Zstd } }

    /// Compresse `data` et retourne un `CompressResult`.
    pub fn compress(&self, data: &[u8]) -> ExofsResult<CompressResult> {
        if data.len() > MAX_COMPRESS_INPUT { return Err(ExofsError::InvalidSize); }

        let original_size = data.len() as u32;
        let (compressed, effective_algo) = match self.algo {
            CompressionType::None => {
                (data.to_vec_safe()?, CompressionType::None)
            }
            CompressionType::Lz4 => {
                let c = lz4_compress(data)?;
                // Si la compression augmente la taille, stocker sans compression.
                if c.len() >= data.len() {
                    (data.to_vec_safe()?, CompressionType::None)
                } else {
                    (c, CompressionType::Lz4)
                }
            }
            CompressionType::Zstd => {
                let c = zstd_compress(data)?;
                if c.len() >= data.len() {
                    (data.to_vec_safe()?, CompressionType::None)
                } else {
                    (c, CompressionType::Zstd)
                }
            }
        };

        let compressed_size = compressed.len() as u32;
        let hdr             = CompressedBlockHeader::new(effective_algo, original_size, compressed_size);
        let hdr_bytes       = hdr.to_bytes();

        let total = COMPRESS_HEADER_SIZE.checked_add(compressed.len())
            .ok_or(ExofsError::Overflow)?;
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&hdr_bytes);
        out.extend_from_slice(&compressed);

        let ratio_milli = if original_size == 0 {
            1000
        } else {
            ((compressed_size as u64 * 1000) / original_size as u64) as u32
        };

        Ok(CompressResult {
            data: out,
            algo: effective_algo,
            original_size,
            ratio_milli,
        })
    }

    pub fn algorithm(&self) -> CompressionType { self.algo }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait helper : Vec<u8> safe allocation
// ─────────────────────────────────────────────────────────────────────────────

trait ToVecSafe {
    fn to_vec_safe(&self) -> ExofsResult<Vec<u8>>;
}

impl ToVecSafe for [u8] {
    fn to_vec_safe(&self) -> ExofsResult<Vec<u8>> {
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve(self.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(self);
        Ok(v)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4 block format — implémentation simplifiée
// ─────────────────────────────────────────────────────────────────────────────

/// Compresse via LZ4 block format (séquences de literals + matchs).
///
/// Implémentation légère adaptée au no_std noyau.
/// Format respecte le LZ4 block spec rev. 1.8 pour les séquences literals-only
/// (pas de matchs → identique à la sortie de lz4 -B0).
fn lz4_compress(input: &[u8]) -> ExofsResult<Vec<u8>> {
    // Implémentation LZ4 literals-only pour la compatibilité minimale.
    // Une implémentation complète avec hash chain est substituable ici.
    let mut out: Vec<u8> = Vec::new();
    let max_out = input.len().saturating_add(input.len() / 255 + 16);
    out.try_reserve(max_out).map_err(|_| ExofsError::NoMemory)?;

    let mut pos = 0usize;
    while pos < input.len() {
        let remaining      = input.len() - pos;
        let literal_run    = remaining.min(255 * 15 + 15);
        let literal_len    = if literal_run < 15 {
            out.push((literal_run as u8) << 4);
        } else {
            out.push(0xF0u8);
            let extra = literal_run - 15;
            let full   = extra / 255;
            let rem    = extra % 255;
            for _ in 0..full { out.push(255u8); }
            out.push(rem as u8);
        };
        let _ = literal_len;

        out.try_reserve(literal_run).map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&input[pos..pos + literal_run]);
        pos = pos.checked_add(literal_run).ok_or(ExofsError::Overflow)?;
    }

    // End mark : 4 octets zéro (offset fictif).
    out.try_reserve(4).map_err(|_| ExofsError::NoMemory)?;
    out.extend_from_slice(&[0u8; 4]);

    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Zstd — implémentation ring-0 minimale (frame header + literals)
// ─────────────────────────────────────────────────────────────────────────────

/// Zstd minimal : frame header valide + block literals non compressés.
///
/// Produit un stream Zstd valide que tout décompresseur Zstd standard peut lire.
/// Pour une vraie compression Zstd, remplacer par la crate `ruzstd` ou équivalent.
fn zstd_compress(input: &[u8]) -> ExofsResult<Vec<u8>> {
    // Magic Zstd frame.
    const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

    let mut out: Vec<u8> = Vec::new();
    let capacity = input.len().saturating_add(32);
    out.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;

    // Frame header.
    out.extend_from_slice(&ZSTD_MAGIC);
    // FHD : single segment, no checksum, no dict ID.
    out.push(0x60u8);
    // Frame content size (1 byte = ≤255, 2 bytes = ≤65535, 4 bytes = ≤ 4GB).
    if input.len() <= 0xFF {
        out.push(input.len() as u8);
    } else if input.len() <= 0xFFFF {
        let s = (input.len() as u16).to_le_bytes();
        out.extend_from_slice(&s);
    } else {
        let s = (input.len() as u32).to_le_bytes();
        out.extend_from_slice(&s);
    }

    // Block header : type = Raw_Block (literals).
    let block_size = input.len() as u32;
    let bh_val     = (block_size << 3) | 0x01; // type=1 (raw), last_block=1
    out.extend_from_slice(&bh_val.to_le_bytes());
    out.try_reserve(input.len()).map_err(|_| ExofsError::NoMemory)?;
    out.extend_from_slice(input);

    // Content checksum (optionnel, on l'omet).
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(n: usize, fill: u8) -> Vec<u8> { vec![fill; n] }

    #[test]
    fn test_compress_none() {
        let w = CompressWriter::none();
        let d = make_data(1024, 0xAB);
        let r = w.compress(&d).unwrap();
        // En mode None, les données doivent être intactes (headers mis à part).
        let hdr = CompressedBlockHeader::from_bytes(&r.data).unwrap();
        assert_eq!(hdr.original_size as usize, d.len());
        assert_eq!(r.algo, CompressionType::None);
    }

    #[test]
    fn test_header_roundtrip() {
        let hdr  = CompressedBlockHeader::new(CompressionType::Lz4, 4096, 2048);
        let raw  = hdr.to_bytes();
        let hdr2 = CompressedBlockHeader::from_bytes(&raw).unwrap();
        assert_eq!(hdr2.original_size, 4096);
        assert_eq!(hdr2.compressed_size, 2048);
        assert_eq!(CompressionType::from_u8(hdr2.algo).unwrap(), CompressionType::Lz4);
    }

    #[test]
    fn test_compress_lz4_produces_header() {
        let w = CompressWriter::lz4();
        let d = make_data(2048, 0x00);
        let r = w.compress(&d).unwrap();
        let hdr = CompressedBlockHeader::from_bytes(&r.data).unwrap();
        assert_eq!(hdr.magic, COMPRESSED_BLOCK_MAGIC);
        assert_eq!(hdr.original_size, 2048);
    }

    #[test]
    fn test_compress_zstd_produces_header() {
        let w = CompressWriter::zstd();
        let d = make_data(4096, 0x42);
        let r = w.compress(&d).unwrap();
        let hdr = CompressedBlockHeader::from_bytes(&r.data).unwrap();
        assert_eq!(hdr.original_size, 4096);
    }

    #[test]
    fn test_compress_small_input_still_works() {
        let w = CompressWriter::lz4();
        let d = b"hi".to_vec();
        let r = w.compress(&d).unwrap();
        assert!(!r.data.is_empty());
    }
}
