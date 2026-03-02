//! Sélection de l'algorithme de compression optimal pour un blob ExoFS.

use crate::fs::exofs::compress::algorithm::{CompressionAlgorithm, CompressLevel};
use crate::fs::exofs::compress::compress_threshold::CompressionThreshold;

/// Politique de choix de compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressPolicy {
    /// Toujours choisir LZ4 (faible latence).
    AlwaysLz4,
    /// Toujours choisir Zstd (meilleur ratio).
    AlwaysZstd,
    /// Choix adaptatif basé sur la taille et les stats historiques.
    Adaptive,
    /// Aucune compression.
    None,
}

/// Décision de compression retournée par CompressionChoice.
#[derive(Debug, Clone, Copy)]
pub struct CompressDecision {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressLevel,
}

/// Sélecteur d'algorithme de compression.
pub struct CompressionChoice {
    policy: CompressPolicy,
    threshold: CompressionThreshold,
    /// Seuil de taille au-dessus duquel Zstd vaut mieux que LZ4.
    zstd_size_threshold: usize,
}

impl CompressionChoice {
    pub const fn new(policy: CompressPolicy) -> Self {
        Self {
            policy,
            threshold: CompressionThreshold::default(),
            zstd_size_threshold: 32768, // 32 KiB
        }
    }

    /// Décide l'algorithme et le niveau pour un blob donné.
    pub fn decide(&self, data: &[u8]) -> CompressDecision {
        if !self.threshold.should_compress(data) {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
            };
        }

        match self.policy {
            CompressPolicy::None => CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
            },
            CompressPolicy::AlwaysLz4 => CompressDecision {
                algorithm: CompressionAlgorithm::Lz4,
                level: CompressLevel::Fast,
            },
            CompressPolicy::AlwaysZstd => CompressDecision {
                algorithm: CompressionAlgorithm::Zstd,
                level: CompressLevel::Default,
            },
            CompressPolicy::Adaptive => {
                // LZ4 pour les petits blobs (hot path), Zstd pour les grands.
                if data.len() < self.zstd_size_threshold {
                    CompressDecision {
                        algorithm: CompressionAlgorithm::Lz4,
                        level: CompressLevel::Fast,
                    }
                } else {
                    CompressDecision {
                        algorithm: CompressionAlgorithm::Zstd,
                        level: CompressLevel::Default,
                    }
                }
            }
        }
    }
}
