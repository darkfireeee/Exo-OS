//! Algorithmes de compression supportés par ExoFS.
//!
//! Définit : algorithmes, niveaux, bornes de buffer, capacités, validation.
//!
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE ONDISK-03 : aucun AtomicU64 dans les types #[repr(C)].

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// CompressLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Niveaux de compression (1 = rapide, 9 = maximum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CompressLevel {
    None = 0,
    Fast = 1,
    Default = 3,
    Best = 6,
    Maximum = 9,
}

impl CompressLevel {
    /// Construit depuis un octet on-disk. Retourne `Default` si invalide.
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Fast,
            3 => Self::Default,
            6 => Self::Best,
            7..=255 => Self::Maximum,
            _ => Self::Default,
        }
    }

    /// Valeur numérique pour les appels de librairie embarquée.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// `true` si le niveau est au moins `Default`.
    pub const fn is_at_least_default(self) -> bool {
        (self as u8) >= (CompressLevel::Default as u8)
    }

    /// Retourne le niveau immédiatement supérieur (plafonné à Maximum).
    pub const fn next(self) -> Self {
        match self {
            Self::None => Self::Fast,
            Self::Fast => Self::Default,
            Self::Default => Self::Best,
            Self::Best => Self::Maximum,
            Self::Maximum => Self::Maximum,
        }
    }

    /// Retourne le niveau immédiatement inférieur (plancher à Fast).
    pub const fn prev(self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Fast => Self::None,
            Self::Default => Self::Fast,
            Self::Best => Self::Default,
            Self::Maximum => Self::Best,
        }
    }

    /// Nom lisible du niveau.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fast => "fast",
            Self::Default => "default",
            Self::Best => "best",
            Self::Maximum => "maximum",
        }
    }

    /// Tous les niveaux valides, du plus rapide au plus lent.
    pub const ALL: [CompressLevel; 4] = [
        CompressLevel::Fast,
        CompressLevel::Default,
        CompressLevel::Best,
        CompressLevel::Maximum,
    ];
}

impl Default for CompressLevel {
    fn default() -> Self {
        CompressLevel::Default
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionAlgorithm
// ─────────────────────────────────────────────────────────────────────────────

/// Algorithme de compression utilisé pour un blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompressionAlgorithm {
    /// Pas de compression.
    None = 0,
    /// LZ4 — rapide, faible ratio, idéal pour données chaudes.
    Lz4 = 1,
    /// Zstd — meilleur ratio, légèrement plus lent.
    Zstd = 2,
}

impl CompressionAlgorithm {
    /// Retourne le nom lisible de l'algorithme.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }

    /// `true` si les données sont effectivement compressées.
    pub const fn is_compressed(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Parse depuis un octet on-disk. Retourne `None` si valeur inconnue.
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            _ => None,
        }
    }

    /// Parse depuis un octet on-disk, retourne `ExofsError::NotSupported` si inconnu.
    pub fn parse_on_disk(v: u8) -> ExofsResult<Self> {
        Self::from_u8(v).ok_or(ExofsError::NotSupported)
    }

    /// Niveau de compression par défaut recommandé pour cet algorithme.
    pub const fn default_level(self) -> CompressLevel {
        match self {
            Self::None => CompressLevel::Default,
            Self::Lz4 => CompressLevel::Fast,
            Self::Zstd => CompressLevel::Default,
        }
    }

    /// `true` si l'algorithme supporte les niveaux multiples.
    pub const fn supports_levels(self) -> bool {
        matches!(self, Self::Zstd)
    }

    /// Latence relative (1 = le plus rapide, copie mémoire de référence).
    pub const fn relative_latency(self) -> u8 {
        match self {
            Self::None => 1,
            Self::Lz4 => 2,
            Self::Zstd => 5,
        }
    }

    /// Tous les algorithmes, y compris None.
    pub const ALL: [CompressionAlgorithm; 3] = [
        CompressionAlgorithm::None,
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Zstd,
    ];

    /// Algorithmes compressants uniquement (hors None).
    pub const COMPRESSING: [CompressionAlgorithm; 2] =
        [CompressionAlgorithm::Lz4, CompressionAlgorithm::Zstd];
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        CompressionAlgorithm::None
    }
}
impl core::convert::TryFrom<u8> for CompressionAlgorithm {
    type Error = ();
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::from_u8(v).ok_or(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de buffer
// ─────────────────────────────────────────────────────────────────────────────

/// LZ4 : sortie max = entrée + entrée/255 + 16 bytes (spécification block format).
pub const LZ4_MAX_EXPANSION_BYTES: usize = 16;

/// Zstd : marge sécurité supplémentaire hors spécification stricte.
pub const ZSTD_COMPRESSBOUND_MARGIN: usize = 128;

/// Taille maximale d'un bloc compressé acceptée en entrée (anti-DoS).
pub const MAX_COMPRESSED_BLOCK_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// Taille maximale d'un bloc décompressé acceptée (anti-DoS).
pub const MAX_DECOMPRESSED_BLOCK_SIZE: usize = 256 * 1024 * 1024; // 256 MiB

/// Taille minimale pour qu'une compression soit tentée.
pub const MIN_COMPRESS_INPUT: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de calcul de bornes
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule la borne supérieure du buffer de sortie.
/// Arithmétique saturating (ARITH-02) : jamais de panique par overflow.
pub fn compress_bound(algorithm: CompressionAlgorithm, input_len: usize) -> usize {
    match algorithm {
        CompressionAlgorithm::None => input_len,
        CompressionAlgorithm::Lz4 => {
            let margin = input_len.saturating_div(255);
            input_len
                .saturating_add(margin)
                .saturating_add(LZ4_MAX_EXPANSION_BYTES)
        }
        CompressionAlgorithm::Zstd => input_len.saturating_add(ZSTD_COMPRESSBOUND_MARGIN),
    }
}

/// Valide qu'une taille décompressée est dans les limites de sécurité.
pub fn validate_decompressed_size(uncompressed_size: u64) -> ExofsResult<usize> {
    let sz = uncompressed_size as usize;
    if sz > MAX_DECOMPRESSED_BLOCK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(sz)
}

/// Valide qu'une taille compressée est dans les limites de sécurité.
pub fn validate_compressed_size(compressed_size: u64) -> ExofsResult<usize> {
    let sz = compressed_size as usize;
    if sz > MAX_COMPRESSED_BLOCK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(sz)
}

// ─────────────────────────────────────────────────────────────────────────────
// AlgorithmCapabilities — capacités déclarées par algorithme
// ─────────────────────────────────────────────────────────────────────────────

/// Capacités d'un algorithme de compression.
#[derive(Debug, Clone, Copy)]
pub struct AlgorithmCapabilities {
    pub algorithm: CompressionAlgorithm,
    pub stream_support: bool,
    pub dict_support: bool,
    pub max_level: CompressLevel,
    /// Débit de compression typique en MB/s.
    pub compress_mbps: u32,
    /// Débit de décompression typique en MB/s.
    pub decompress_mbps: u32,
}

impl AlgorithmCapabilities {
    /// Retourne les capacités de l'algorithme donné.
    pub const fn for_algorithm(algo: CompressionAlgorithm) -> Self {
        match algo {
            CompressionAlgorithm::None => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::None,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Default,
                compress_mbps: 10_000,
                decompress_mbps: 10_000,
            },
            CompressionAlgorithm::Lz4 => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::Lz4,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Fast,
                compress_mbps: 600,
                decompress_mbps: 3_000,
            },
            CompressionAlgorithm::Zstd => AlgorithmCapabilities {
                algorithm: CompressionAlgorithm::Zstd,
                stream_support: false,
                dict_support: false,
                max_level: CompressLevel::Maximum,
                compress_mbps: 400,
                decompress_mbps: 1_500,
            },
        }
    }

    /// `true` si l'algorithme peut être utilisé pour ce niveau.
    pub fn level_supported(&self, level: CompressLevel) -> bool {
        level <= self.max_level
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionProfile — couple (algorithme, niveau) persistable
// ─────────────────────────────────────────────────────────────────────────────

/// Profil de compression sérialisable sur 2 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionProfile {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressLevel,
}

impl CompressionProfile {
    /// Profil par défaut : Lz4 en mode Fast.
    pub const fn default_lz4() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Lz4,
            level: CompressLevel::Fast,
        }
    }

    /// Profil haute compression : Zstd Default.
    pub const fn high_ratio() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            level: CompressLevel::Default,
        }
    }

    /// Pas de compression.
    pub const fn none() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: CompressLevel::Default,
        }
    }

    /// Sérialise sur 2 bytes (algorithm, level).
    pub const fn to_bytes(self) -> [u8; 2] {
        [self.algorithm as u8, self.level as u8]
    }

    /// Désérialise depuis 2 bytes.
    pub fn from_bytes(b: [u8; 2]) -> ExofsResult<Self> {
        let algorithm = CompressionAlgorithm::parse_on_disk(b[0])?;
        let level = CompressLevel::from_u8(b[1]);
        Ok(Self { algorithm, level })
    }
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self::default_lz4()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_algorithm_from_u8_all() {
        assert_eq!(
            CompressionAlgorithm::from_u8(0),
            Some(CompressionAlgorithm::None)
        );
        assert_eq!(
            CompressionAlgorithm::from_u8(1),
            Some(CompressionAlgorithm::Lz4)
        );
        assert_eq!(
            CompressionAlgorithm::from_u8(2),
            Some(CompressionAlgorithm::Zstd)
        );
        assert_eq!(CompressionAlgorithm::from_u8(3), None);
        assert_eq!(CompressionAlgorithm::from_u8(255), None);
    }

    #[test]
    fn test_parse_on_disk_invalid_returns_not_supported() {
        assert_eq!(
            CompressionAlgorithm::parse_on_disk(42),
            Err(ExofsError::NotSupported)
        );
    }

    #[test]
    fn test_level_from_u8_edges() {
        assert_eq!(CompressLevel::from_u8(1), CompressLevel::Fast);
        assert_eq!(CompressLevel::from_u8(3), CompressLevel::Default);
        assert_eq!(CompressLevel::from_u8(6), CompressLevel::Best);
        assert_eq!(CompressLevel::from_u8(9), CompressLevel::Maximum);
        assert_eq!(CompressLevel::from_u8(0), CompressLevel::Default); // invalid → Default
    }

    #[test]
    fn test_compress_bound_lz4() {
        let n = compress_bound(CompressionAlgorithm::Lz4, 1000);
        assert!(n >= 1000 + 16);
    }

    #[test]
    fn test_compress_bound_none_identity() {
        assert_eq!(compress_bound(CompressionAlgorithm::None, 500), 500);
    }

    #[test]
    fn test_compress_bound_no_overflow() {
        // saturating_add : jamais de panic même avec max usize
        let n = compress_bound(CompressionAlgorithm::Zstd, usize::MAX - 256);
        assert!(n > 0);
    }

    #[test]
    fn test_validate_decompressed_size_ok() {
        assert!(validate_decompressed_size(1024).is_ok());
        assert_eq!(validate_decompressed_size(1024).unwrap(), 1024);
    }

    #[test]
    fn test_validate_decompressed_size_over_limit() {
        let over = (MAX_DECOMPRESSED_BLOCK_SIZE + 1) as u64;
        assert_eq!(
            validate_decompressed_size(over),
            Err(ExofsError::InvalidArgument)
        );
    }

    #[test]
    fn test_validate_compressed_size_over_limit() {
        let over = (MAX_COMPRESSED_BLOCK_SIZE + 1) as u64;
        assert_eq!(
            validate_compressed_size(over),
            Err(ExofsError::InvalidArgument)
        );
    }

    #[test]
    fn test_level_next_chain() {
        assert_eq!(CompressLevel::Fast.next(), CompressLevel::Default);
        assert_eq!(CompressLevel::Default.next(), CompressLevel::Best);
        assert_eq!(CompressLevel::Best.next(), CompressLevel::Maximum);
        assert_eq!(CompressLevel::Maximum.next(), CompressLevel::Maximum);
    }

    #[test]
    fn test_level_prev_chain() {
        assert_eq!(CompressLevel::Maximum.prev(), CompressLevel::Best);
        assert_eq!(CompressLevel::Best.prev(), CompressLevel::Default);
        assert_eq!(CompressLevel::Default.prev(), CompressLevel::Fast);
        assert_eq!(CompressLevel::Fast.prev(), CompressLevel::Fast);
    }

    #[test]
    fn test_capabilities_lz4_decompresses_faster() {
        let caps = AlgorithmCapabilities::for_algorithm(CompressionAlgorithm::Lz4);
        assert!(caps.decompress_mbps > caps.compress_mbps);
    }

    #[test]
    fn test_algorithm_is_compressed() {
        assert!(!CompressionAlgorithm::None.is_compressed());
        assert!(CompressionAlgorithm::Lz4.is_compressed());
        assert!(CompressionAlgorithm::Zstd.is_compressed());
    }

    #[test]
    fn test_compression_profile_roundtrip() {
        let p = CompressionProfile::default_lz4();
        let b = p.to_bytes();
        let p2 = CompressionProfile::from_bytes(b).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn test_compression_profile_high_ratio() {
        let p = CompressionProfile::high_ratio();
        assert_eq!(p.algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(p.level, CompressLevel::Default);
    }

    #[test]
    fn test_level_is_at_least_default() {
        assert!(!CompressLevel::Fast.is_at_least_default());
        assert!(CompressLevel::Default.is_at_least_default());
        assert!(CompressLevel::Maximum.is_at_least_default());
    }

    #[test]
    fn test_algorithm_supports_levels() {
        assert!(!CompressionAlgorithm::Lz4.supports_levels());
        assert!(CompressionAlgorithm::Zstd.supports_levels());
    }
}
