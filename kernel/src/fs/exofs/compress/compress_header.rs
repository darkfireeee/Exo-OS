//! En-tête de compression on-disk ExoFS.
//!
//! RÈGLE 4  : struct on-disk → #[repr(C)] + const assert taille.
//! RÈGLE 8  : vérifie magic EN PREMIER dans tout parsing on-disk.

use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::core::FsError;

/// Magic de l'en-tête de compression.
pub const COMPRESSION_MAGIC: u32 = 0xC0_4D_50_52; // "CMPR"

/// En-tête on-disk précédant les données compressées d'un blob.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CompressionHeader {
    /// Magic : COMPRESSION_MAGIC.
    pub magic: u32,
    /// Algorithme utilisé (1 byte de l'enum).
    pub algorithm: u8,
    /// Niveau de compression.
    pub level: u8,
    /// Réservé pour alignement futur.
    _pad: [u8; 2],
    /// Taille des données décompressées.
    pub uncompressed_size: u64,
    /// Taille des données compressées (sans cet en-tête).
    pub compressed_size: u64,
    /// Checksum CRC32 des données compressées.
    pub crc32: u32,
    /// Réservé.
    _reserved: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<CompressionHeader>() == 32,
    "CompressionHeader doit faire exactement 32 bytes"
);

impl CompressionHeader {
    /// Crée un en-tête valide.
    pub fn new(
        algorithm: CompressionAlgorithm,
        level: u8,
        uncompressed_size: u64,
        compressed_size: u64,
        crc32: u32,
    ) -> Self {
        Self {
            magic: COMPRESSION_MAGIC,
            algorithm: algorithm as u8,
            level,
            _pad: [0; 2],
            uncompressed_size,
            compressed_size,
            crc32,
            _reserved: [0; 4],
        }
    }

    /// Parse depuis un buffer on-disk. RÈGLE 8 : magic EN PREMIER.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, FsError> {
        if buf.len() < core::mem::size_of::<Self>() {
            return Err(FsError::CorruptData);
        }
        // RÈGLE 8 : magic EN PREMIER.
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != COMPRESSION_MAGIC {
            return Err(FsError::BadMagic);
        }
        // SAFETY: buf est aligné et de taille suffisante.
        let header: Self = unsafe {
            core::ptr::read_unaligned(buf.as_ptr() as *const Self)
        };
        // Vérifie l'algorithme.
        if CompressionAlgorithm::from_u8(header.algorithm).is_none() {
            return Err(FsError::UnsupportedAlgorithm);
        }
        Ok(header)
    }

    /// Serialise en bytes on-disk.
    pub fn to_bytes(&self) -> [u8; 32] {
        // SAFETY: Self est #[repr(C)] Plain Old Data de taille 32.
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Algorithme parsé.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::from_u8(self.algorithm).unwrap_or(CompressionAlgorithm::None)
    }
}
