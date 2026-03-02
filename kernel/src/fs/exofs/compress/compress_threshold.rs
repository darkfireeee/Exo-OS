//! Seuils de compression ExoFS — décide si un blob doit être compressé.
//!
//! Skip la compression si : taille trop petite, ratio estimé trop faible,
//! ou données déjà compressées (magie détectée).

/// Seuil minimum en bytes pour tenter la compression.
pub const MIN_COMPRESS_SIZE: usize = 512;
/// Si le ratio compressé/décompressé dépasse ce seuil (%), on stocke brut.
pub const MAX_RATIO_PERCENT: u64 = 95;

/// Gestion des seuils décidant si la compression est bénéfique.
pub struct CompressionThreshold {
    /// Taille minimum pour tenter la compression.
    pub min_size: usize,
    /// Ratio max acceptable : si `compressed_size / original_size > ratio_threshold`, skip.
    pub ratio_threshold: u64,
    /// Activer la détection des données déjà compressées.
    pub detect_already_compressed: bool,
}

impl CompressionThreshold {
    pub const fn default() -> Self {
        Self {
            min_size: MIN_COMPRESS_SIZE,
            ratio_threshold: MAX_RATIO_PERCENT,
            detect_already_compressed: true,
        }
    }

    /// `true` si la compression doit être tentée pour ces données.
    pub fn should_compress(&self, data: &[u8]) -> bool {
        if data.len() < self.min_size {
            return false;
        }
        if self.detect_already_compressed && looks_compressed(data) {
            return false;
        }
        true
    }

    /// `true` si le résultat compressé est suffisamment meilleur que l'original.
    pub fn is_worth_storing(&self, compressed_len: usize, original_len: usize) -> bool {
        if original_len == 0 {
            return false;
        }
        let ratio = (compressed_len as u64 * 100) / (original_len as u64);
        ratio <= self.ratio_threshold
    }
}

/// Détecte heuristiquement si les données semblent déjà compressées.
fn looks_compressed(data: &[u8]) -> bool {
    // Vérifie les magics communs : Zstd, LZ4, GZIP, ZIP, BZIP2, XZ.
    if data.len() < 4 {
        return false;
    }
    let magic4 = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    matches!(
        magic4,
        0xFD2F_B528 // Zstd
        | 0x0400_2270 // LZ4 frame
        | 0x1F8B_0800 // GZIP
        | 0x504B_0304 // ZIP
        | 0x425A_6839 // BZIP2
        | 0xFD37_7A58 // XZ
    )
}
