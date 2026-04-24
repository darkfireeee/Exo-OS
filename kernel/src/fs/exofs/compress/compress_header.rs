//! En-tête de compression on-disk ExoFS.
//!
//! Structure persistante 32-bytes précédant chaque blob compressé.
//!
//! RÈGLE ONDISK-03 : #[repr(C)], taille assertée, aucun AtomicU64.
//! RÈGLE ARITH-02  : arithmétique checked/saturating uniquement.
//! RÈGLE MAGIC-01  : vérification du magic EN PREMIER dans tout parsing on-disk.

use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic de l'en-tête de compression ExoFS ("CMPR" little-endian).
pub const COMPRESSION_MAGIC: u32 = 0xC0_4D_50_52;

/// Version actuelle du format d'en-tête.
pub const HEADER_VERSION: u8 = 1;

/// Taille fixe de l'en-tête on-disk.
pub const COMPRESSION_HEADER_SIZE: usize = 32;

/// En-tête on-disk précédant les données compressées d'un blob ExoFS.
///
/// Layout mémoire (32 bytes) :
/// ```text
/// offset  size  champ
///  0       4    magic
///  4       1    algorithm
///  5       1    level
///  6       1    version
///  7       1    flags
///  8       8    original_size
/// 16       8    compressed_size
/// 24       4    crc32
/// 28       4    _reserved
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CompressionHeader {
    /// Magic : doit être COMPRESSION_MAGIC. Vérifié EN PREMIER.
    pub magic: u32,
    /// Algorithme utilisé (CompressionAlgorithm as u8).
    pub algorithm: u8,
    /// Niveau de compression.
    pub level: u8,
    /// Version du format d'en-tête.
    pub version: u8,
    /// Flags réservés pour usage futur.
    pub flags: u8,
    /// Taille des données décompressées en bytes.
    pub original_size: u64,
    /// Taille des données compressées (sans cet en-tête).
    pub compressed_size: u64,
    /// Checksum CRC32 des données compressées.
    pub crc32: u32,
    /// Réservé pour alignement futur.
    pub _reserved: [u8; 4],
}

// Vérification statique de taille on-disk (ONDISK-03).
const _: () = assert!(
    core::mem::size_of::<CompressionHeader>() == COMPRESSION_HEADER_SIZE,
    "CompressionHeader doit faire exactement 32 bytes"
);

impl CompressionHeader {
    /// Crée un en-tête valide avec les paramètres donnés.
    pub fn new(
        algorithm: CompressionAlgorithm,
        level: u8,
        original_size: u64,
        compressed_size: u64,
        crc32: u32,
    ) -> Self {
        Self {
            magic: COMPRESSION_MAGIC,
            algorithm: algorithm as u8,
            level,
            version: HEADER_VERSION,
            flags: 0,
            original_size,
            compressed_size,
            crc32,
            _reserved: [0; 4],
        }
    }

    /// Parse depuis un buffer on-disk. MAGIC-01 : magic vérifié EN PREMIER.
    pub fn from_bytes(buf: &[u8]) -> ExofsResult<Self> {
        if buf.len() < COMPRESSION_HEADER_SIZE {
            return Err(ExofsError::CorruptedStructure);
        }
        // MAGIC-01 : magic EN PREMIER avant tout autre champ.
        let magic = u32::from_le_bytes(
            buf[0..4]
                .try_into()
                .map_err(|_| ExofsError::CorruptedStructure)?,
        );
        if magic != COMPRESSION_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // Vérifie l'algorithme.
        let algo_byte = buf[4];
        if CompressionAlgorithm::from_u8(algo_byte).is_none() {
            return Err(ExofsError::NotSupported);
        }
        // Vérifie la version.
        let version = buf[6];
        if version > HEADER_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        // SAFETY: buf est de taille suffisante, repr(C) Plain Old Data.
        let header: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        Ok(header)
    }

    /// Sérialise en bytes on-disk (32 bytes).
    ///
    /// SAFETY: Self est #[repr(C)], Plain Old Data, taille assertée 32.
    pub fn to_bytes(&self) -> [u8; COMPRESSION_HEADER_SIZE] {
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Retourne l'algorithme parsé.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::from_u8(self.algorithm).unwrap_or(CompressionAlgorithm::None)
    }

    /// `true` si l'en-tête est structurellement valide.
    pub fn is_valid(&self) -> bool {
        self.magic == COMPRESSION_MAGIC
            && CompressionAlgorithm::from_u8(self.algorithm).is_some()
            && self.version <= HEADER_VERSION
    }

    /// Vérifie que `compressed_size` cohère avec le buffer payload fourni.
    pub fn validate_payload_len(&self, payload_len: usize) -> ExofsResult<()> {
        let expected = self.compressed_size as usize;
        if payload_len < expected {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(())
    }

    /// Taille totale on-disk (en-tête + données).
    /// Arithmétique checked (ARITH-02).
    pub fn total_on_disk_size(&self) -> Option<u64> {
        (COMPRESSION_HEADER_SIZE as u64).checked_add(self.compressed_size)
    }

    /// `true` si les données sont compressées (algorithme != None).
    pub fn is_compressed(&self) -> bool {
        self.algorithm() != CompressionAlgorithm::None
    }

    /// Retourne les bytes du payload depuis un buffer complet.
    pub fn payload<'a>(&self, full_buf: &'a [u8]) -> ExofsResult<&'a [u8]> {
        let start = COMPRESSION_HEADER_SIZE;
        let end = start
            .checked_add(self.compressed_size as usize)
            .ok_or(ExofsError::OffsetOverflow)?;
        full_buf
            .get(start..end)
            .ok_or(ExofsError::CorruptedStructure)
    }
}

impl Default for CompressionHeader {
    fn default() -> Self {
        Self::new(CompressionAlgorithm::None, 0, 0, 0, 0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressedBlobView — vue structurée d'un blob compressé
// ─────────────────────────────────────────────────────────────────────────────

/// Vue structurée d'un buffer (en-tête parsé + payload emprunté).
pub struct CompressedBlobView<'a> {
    pub header: CompressionHeader,
    pub payload: &'a [u8],
}

impl<'a> CompressedBlobView<'a> {
    /// Parse depuis un buffer complet (en-tête + payload).
    /// Valide le magic, l'algorithme, la cohérence des tailles.
    pub fn parse(buf: &'a [u8]) -> ExofsResult<Self> {
        let header = CompressionHeader::from_bytes(buf)?;
        let payload = header.payload(buf)?;
        Ok(Self { header, payload })
    }

    /// Taille originale des données (avant compression).
    pub fn original_size(&self) -> u64 {
        self.header.original_size
    }

    /// Algorithme utilisé pour ce blob.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        self.header.algorithm()
    }

    /// Checksum CRC32 des données compressées.
    pub fn crc32(&self) -> u32 {
        self.header.crc32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn make_valid_header_bytes(compressed_size: u64) -> [u8; 32] {
        let h = CompressionHeader::new(
            CompressionAlgorithm::Lz4,
            1,
            1000,
            compressed_size,
            0xDEAD_BEEF,
        );
        h.to_bytes()
    }

    #[test]
    fn test_header_size_is_32() {
        assert_eq!(core::mem::size_of::<CompressionHeader>(), 32);
    }

    #[test]
    fn test_new_and_is_valid() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 1000, 400, 0);
        assert!(h.is_valid());
        assert_eq!(h.magic, COMPRESSION_MAGIC);
        assert_eq!(h.version, HEADER_VERSION);
    }

    #[test]
    fn test_roundtrip_to_from_bytes() {
        let h = CompressionHeader::new(CompressionAlgorithm::Zstd, 3, 2048, 512, 0xABCD);
        let bytes = h.to_bytes();
        let h2 = CompressionHeader::from_bytes(&bytes).unwrap();
        assert_eq!(h.magic, h2.magic);
        assert_eq!(h.algorithm, h2.algorithm);
        assert_eq!(h.original_size, h2.original_size);
        assert_eq!(h.compressed_size, h2.compressed_size);
        assert_eq!(h.crc32, h2.crc32);
    }

    #[test]
    fn test_from_bytes_bad_magic() {
        let mut buf = make_valid_header_bytes(400);
        buf[0] = 0xFF;
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::InvalidMagic)
        ));
    }

    #[test]
    fn test_from_bytes_bad_algorithm() {
        let mut buf = make_valid_header_bytes(400);
        buf[4] = 0xFF;
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::NotSupported)
        ));
    }

    #[test]
    fn test_from_bytes_too_short() {
        let buf = [0u8; 10];
        assert!(matches!(
            CompressionHeader::from_bytes(&buf),
            Err(ExofsError::CorruptedStructure)
        ));
    }

    #[test]
    fn test_algorithm_accessor() {
        let h = CompressionHeader::new(CompressionAlgorithm::Zstd, 3, 100, 50, 0);
        assert_eq!(h.algorithm(), CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_total_on_disk_size() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 1000, 400, 0);
        assert_eq!(h.total_on_disk_size(), Some(432));
    }

    #[test]
    fn test_total_on_disk_size_overflow() {
        let h = CompressionHeader::new(CompressionAlgorithm::None, 0, 0, u64::MAX, 0);
        assert_eq!(h.total_on_disk_size(), None);
    }

    #[test]
    fn test_is_compressed_none() {
        let h = CompressionHeader::new(CompressionAlgorithm::None, 0, 100, 100, 0);
        assert!(!h.is_compressed());
    }

    #[test]
    fn test_is_compressed_lz4() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert!(h.is_compressed());
    }

    #[test]
    fn test_validate_payload_len_ok() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert!(h.validate_payload_len(50).is_ok());
    }

    #[test]
    fn test_validate_payload_len_too_short() {
        let h = CompressionHeader::new(CompressionAlgorithm::Lz4, 1, 100, 50, 0);
        assert_eq!(
            h.validate_payload_len(49),
            Err(ExofsError::CorruptedStructure)
        );
    }

    #[test]
    fn test_blob_view_parse_valid() {
        let h_bytes = make_valid_header_bytes(10);
        let mut buf = vec![0u8; 32 + 10];
        buf[..32].copy_from_slice(&h_bytes);
        let view = CompressedBlobView::parse(&buf).unwrap();
        assert_eq!(view.algorithm(), CompressionAlgorithm::Lz4);
        assert_eq!(view.original_size(), 1000);
    }

    #[test]
    fn test_blob_view_parse_bad_magic() {
        let mut buf = vec![0u8; 32];
        buf[0] = 0xAA;
        match CompressedBlobView::parse(&buf) {
            Err(e) => assert_eq!(e, ExofsError::InvalidMagic),
            Ok(_) => panic!("expected InvalidMagic"),
        }
    }

    #[test]
    fn test_default_header_none_algo() {
        let h = CompressionHeader::default();
        assert_eq!(h.algorithm(), CompressionAlgorithm::None);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HeaderBuilder — constructeur fluent pour les tests et le code de compression
// ─────────────────────────────────────────────────────────────────────────────

/// Constructeur fluent pour `CompressionHeader`.
pub struct HeaderBuilder {
    algorithm: CompressionAlgorithm,
    level: u8,
    original_size: u64,
    compressed_size: u64,
    crc32: u32,
}

impl HeaderBuilder {
    pub const fn new() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: 1,
            original_size: 0,
            compressed_size: 0,
            crc32: 0,
        }
    }

    pub const fn algorithm(mut self, a: CompressionAlgorithm) -> Self {
        self.algorithm = a;
        self
    }
    pub const fn level(mut self, l: u8) -> Self {
        self.level = l;
        self
    }
    pub const fn uncompressed(mut self, n: u64) -> Self {
        self.original_size = n;
        self
    }
    pub const fn compressed(mut self, n: u64) -> Self {
        self.compressed_size = n;
        self
    }
    pub const fn crc32(mut self, c: u32) -> Self {
        self.crc32 = c;
        self
    }

    pub fn build(self) -> CompressionHeader {
        CompressionHeader::new(
            self.algorithm,
            self.level,
            self.original_size,
            self.compressed_size,
            self.crc32,
        )
    }
}

impl Default for HeaderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod header_builder_tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let h = HeaderBuilder::new()
            .algorithm(CompressionAlgorithm::Zstd)
            .level(3)
            .uncompressed(2048)
            .compressed(512)
            .crc32(0xCAFE_BABE)
            .build();
        assert_eq!(h.algorithm(), CompressionAlgorithm::Zstd);
        assert_eq!(h.original_size, 2048);
        assert_eq!(h.compressed_size, 512);
        assert_eq!(h.crc32, 0xCAFE_BABE);
    }

    #[test]
    fn test_builder_roundtrip() {
        let h = HeaderBuilder::new()
            .algorithm(CompressionAlgorithm::Lz4)
            .uncompressed(100)
            .compressed(50)
            .build();
        let b = h.to_bytes();
        let h2 = CompressionHeader::from_bytes(&b).unwrap();
        assert!(h2.is_valid());
        assert_eq!(h2.algorithm(), CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_version_in_bytes() {
        let h = HeaderBuilder::new().build();
        let b = h.to_bytes();
        assert_eq!(b[6], HEADER_VERSION); // offset 6 = version
    }
}
