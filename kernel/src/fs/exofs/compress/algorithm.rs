//! Algorithmes de compression supportés par ExoFS.

/// Niveaux de compression (1 = rapide, 9 = maximum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CompressLevel {
    Fast    = 1,
    Default = 3,
    Best    = 6,
    Maximum = 9,
}

impl CompressLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Fast,
            3 => Self::Default,
            6 => Self::Best,
            7..=255 => Self::Maximum,
            _ => Self::Default,
        }
    }
}

/// Algorithme de compression utilisé pour un blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompressionAlgorithm {
    /// Pas de compression.
    None  = 0,
    /// LZ4 — rapide, faible ratio, idéal pour données chaudes.
    Lz4   = 1,
    /// Zstd — meilleur ratio, légèrement plus lent.
    Zstd  = 2,
}

impl CompressionAlgorithm {
    /// Retourne le nom lisible de l'algorithme.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4  => "lz4",
            Self::Zstd => "zstd",
        }
    }

    /// Retourne `true` si les données sont effectivement compressées.
    pub fn is_compressed(self) -> bool {
        self != Self::None
    }

    /// Parses depuis un octet on-disk.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            _ => None,
        }
    }
}

/// Capacités maximales de compression pour le dimensionnement des buffers.
/// LZ4 peut théoriquement augmenter la taille de `input + input/255 + 16`.
pub const LZ4_MAX_EXPANSION_BYTES: usize = 16;
/// Zstd ne peut jamais augmenter les données de plus que cette constante.
pub const ZSTD_COMPRESSBOUND_MARGIN: usize = 128;

/// Calcule la borne max du buffer de sortie pour la compression.
pub fn compress_bound(algorithm: CompressionAlgorithm, input_len: usize) -> usize {
    match algorithm {
        CompressionAlgorithm::None => input_len,
        CompressionAlgorithm::Lz4  => input_len + (input_len / 255) + LZ4_MAX_EXPANSION_BYTES,
        CompressionAlgorithm::Zstd => input_len + ZSTD_COMPRESSBOUND_MARGIN,
    }
}
