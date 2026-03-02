//! compression_choice.rs — Sélection de l'algorithme de compression pour le storage (no_std).

/// Algorithme de compression disponible.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    None  = 0,
    Lz4   = 1,
    Zstd  = 2,
}

impl CompressionAlgorithm {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Lz4,
            2 => Self::Zstd,
            _ => Self::None,
        }
    }
}

/// Décide de l'algorithme à utiliser en fonction du type de données et de sa taille.
pub fn choose_algorithm(data_size: usize, entropy_hint: u8) -> CompressionAlgorithm {
    // Pas de compression pour les très petits blocs ou données à haute entropie.
    if data_size < 512 { return CompressionAlgorithm::None; }
    if entropy_hint > 240 { return CompressionAlgorithm::None; } // Données quasi-aléatoires.

    if data_size < 65536 {
        // Lz4 : plus rapide, meilleur pour petits blocs.
        CompressionAlgorithm::Lz4
    } else {
        // Zstd : meilleure compression pour les grands blocs.
        CompressionAlgorithm::Zstd
    }
}

/// Estime l'entropie d'un bloc (0=entropie zéro, 255=max aléatoire).
pub fn estimate_entropy(data: &[u8]) -> u8 {
    if data.is_empty() { return 0; }
    let sample = &data[..data.len().min(256)];
    let mut counts = [0u32; 256];
    for &b in sample { counts[b as usize] += 1; }
    let n = sample.len() as f64;
    // Approximation : proportion de valeurs uniques.
    let unique = counts.iter().filter(|&&c| c > 0).count();
    ((unique * 255) / 256) as u8
}
