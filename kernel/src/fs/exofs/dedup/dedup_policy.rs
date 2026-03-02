//! DedupPolicy — politique de déduplication ExoFS (no_std).

/// Mode de déduplication.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DedupMode {
    Disabled   = 0,
    FixedSize  = 1,   // Chunks de taille fixe.
    Cdc        = 2,   // Content-Defined Chunking (Rabin).
    Adaptive   = 3,   // CDC pour grands fichiers, fixe pour petits.
}

/// Politique complète de déduplication.
#[derive(Clone, Debug)]
pub struct DedupPolicy {
    pub mode:              DedupMode,
    pub min_blob_size:     u64,   // Ne pas tenter dedup si < threshold.
    pub chunk_min:         u32,
    pub chunk_avg:         u32,
    pub chunk_max:         u32,
    pub similarity_thresh: u8,    // % de similarité minimum pour match (0-100).
    pub inline_dedup:      bool,  // Dédup synchrone à l'écriture.
    pub background_dedup:  bool,  // Dédup asynchrone en arrière-plan.
}

impl DedupPolicy {
    /// Politique par défaut : CDC adaptatif, seuil 4 KiB.
    pub fn default_adaptive() -> Self {
        Self {
            mode:              DedupMode::Adaptive,
            min_blob_size:     4096,
            chunk_min:         2048,
            chunk_avg:         8192,
            chunk_max:         65536,
            similarity_thresh: 70,
            inline_dedup:      true,
            background_dedup:  true,
        }
    }

    /// Politique désactivée (bypass complet).
    pub fn disabled() -> Self {
        Self {
            mode: DedupMode::Disabled,
            min_blob_size: u64::MAX,
            chunk_min: 4096, chunk_avg: 4096, chunk_max: 4096,
            similarity_thresh: 100,
            inline_dedup: false, background_dedup: false,
        }
    }

    /// Taille fixe (4K).
    pub fn fixed_4k() -> Self {
        Self {
            mode: DedupMode::FixedSize,
            min_blob_size: 4096,
            chunk_min: 4096, chunk_avg: 4096, chunk_max: 4096,
            similarity_thresh: 80,
            inline_dedup: true,
            background_dedup: false,
        }
    }

    pub fn should_dedup(&self, data_size: u64) -> bool {
        self.mode != DedupMode::Disabled && data_size >= self.min_blob_size
    }
}
