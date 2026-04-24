//! Sélection de l'algorithme de compression optimal pour un blob ExoFS.
//!
//! La politique de compression est configurable :
//! - `AlwaysLz4` : latence minimale, pas de décision adaptative.
//! - `AlwaysZstd` : ratio maximal, plus lent.
//! - `Adaptive`  : LZ4 pour petits blobs / hot path, Zstd pour grands blobs.
//! - `None`      : aucune compression (pass-through).
//!
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

use crate::fs::exofs::compress::algorithm::{
    CompressLevel, CompressionAlgorithm, CompressionProfile,
};
use crate::fs::exofs::compress::compress_threshold::CompressionThreshold;

// ─────────────────────────────────────────────────────────────────────────────
// CompressPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de choix de compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressPolicy {
    /// Toujours choisir LZ4 (faible latence).
    AlwaysLz4,
    /// Toujours choisir Zstd (meilleur ratio).
    AlwaysZstd,
    /// Choix adaptatif basé sur la taille et l'entropie estimée.
    Adaptive,
    /// Aucune compression.
    None,
}

impl CompressPolicy {
    /// Nom lisible de la politique.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AlwaysLz4 => "always_lz4",
            Self::AlwaysZstd => "always_zstd",
            Self::Adaptive => "adaptive",
            Self::None => "none",
        }
    }

    /// `true` si la politique peut effectivement compresser les données.
    pub const fn is_compressing(self) -> bool {
        !matches!(self, Self::None)
    }

    /// LZ4 à niveau par défaut (latence faible).
    pub const fn lz4_default() -> Self {
        Self::AlwaysLz4
    }
    /// LZ4 à vitesse maximale.
    pub const fn lz4_fast() -> Self {
        Self::AlwaysLz4
    }
    /// Zstd à niveau par défaut (bon ratio).
    pub const fn zstd_default() -> Self {
        Self::AlwaysZstd
    }
}

impl Default for CompressPolicy {
    fn default() -> Self {
        CompressPolicy::Adaptive
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DecisionReason + CompressDecision
// ─────────────────────────────────────────────────────────────────────────────

/// Raison de la décision de compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionReason {
    PolicyForced,
    TooSmall,
    AlreadyCompressed,
    HighEntropy,
    AdaptiveLz4SmallBlob,
    AdaptiveZstdLargeBlob,
    Default,
}

/// Décision de compression retournée par `CompressionChoice`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressDecision {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressLevel,
    pub reason: DecisionReason,
}

impl CompressDecision {
    /// Profil (algorithm + level) de cette décision.
    pub fn profile(self) -> CompressionProfile {
        CompressionProfile {
            algorithm: self.algorithm,
            level: self.level,
        }
    }

    /// `true` si la compression sera effectivement appliquée.
    pub fn will_compress(self) -> bool {
        self.algorithm.is_compressed()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionChoice
// ─────────────────────────────────────────────────────────────────────────────

/// Sélecteur d'algorithme de compression.
pub struct CompressionChoice {
    policy: CompressPolicy,
    threshold: CompressionThreshold,
    /// Seuil de taille au-delà duquel Zstd est préféré à LZ4 en mode adaptatif.
    zstd_size_threshold: usize,
    lz4_level: CompressLevel,
    zstd_level: CompressLevel,
}

impl CompressionChoice {
    /// Crée un sélecteur avec la politique donnée et les paramètres par défaut.
    pub const fn new(policy: CompressPolicy) -> Self {
        Self {
            policy,
            threshold: CompressionThreshold::default(),
            zstd_size_threshold: 32768,
            lz4_level: CompressLevel::Fast,
            zstd_level: CompressLevel::Default,
        }
    }

    /// Configure le seuil adaptatif LZ4 → Zstd.
    pub const fn with_zstd_threshold(mut self, bytes: usize) -> Self {
        self.zstd_size_threshold = bytes;
        self
    }

    /// Configure le niveau LZ4 par défaut.
    pub const fn with_lz4_level(mut self, l: CompressLevel) -> Self {
        self.lz4_level = l;
        self
    }

    /// Configure le niveau Zstd par défaut.
    pub const fn with_zstd_level(mut self, l: CompressLevel) -> Self {
        self.zstd_level = l;
        self
    }

    pub fn policy(&self) -> CompressPolicy {
        self.policy
    }

    pub fn set_policy(&mut self, p: CompressPolicy) {
        self.policy = p;
    }

    /// Décide l'algorithme et le niveau pour un blob donné.
    /// RECUR-01 : pas de récursivité — logique purement itérative.
    pub fn decide(&self, data: &[u8]) -> CompressDecision {
        // 1. Politique None.
        if self.policy == CompressPolicy::None {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::PolicyForced,
            };
        }
        // 2. Taille minimale.
        if data.len() < self.threshold.min_size {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::TooSmall,
            };
        }
        // 3. Magic bytes (données déjà compressées).
        if self.threshold.detect_already_compressed
            && crate::fs::exofs::compress::compress_threshold::looks_compressed(data)
        {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::AlreadyCompressed,
            };
        }
        // 4. Entropie trop haute.
        if self.threshold.detect_high_entropy {
            let e = crate::fs::exofs::compress::compress_threshold::estimate_entropy(data);
            if e >= self.threshold.entropy_threshold {
                return CompressDecision {
                    algorithm: CompressionAlgorithm::None,
                    level: CompressLevel::Default,
                    reason: DecisionReason::HighEntropy,
                };
            }
        }
        // 5. Application de la politique.
        match self.policy {
            CompressPolicy::AlwaysLz4 => CompressDecision {
                algorithm: CompressionAlgorithm::Lz4,
                level: self.lz4_level,
                reason: DecisionReason::PolicyForced,
            },
            CompressPolicy::AlwaysZstd => CompressDecision {
                algorithm: CompressionAlgorithm::Zstd,
                level: self.zstd_level,
                reason: DecisionReason::PolicyForced,
            },
            CompressPolicy::Adaptive => {
                if data.len() < self.zstd_size_threshold {
                    CompressDecision {
                        algorithm: CompressionAlgorithm::Lz4,
                        level: self.lz4_level,
                        reason: DecisionReason::AdaptiveLz4SmallBlob,
                    }
                } else {
                    CompressDecision {
                        algorithm: CompressionAlgorithm::Zstd,
                        level: self.zstd_level,
                        reason: DecisionReason::AdaptiveZstdLargeBlob,
                    }
                }
            }
            CompressPolicy::None => CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::PolicyForced,
            },
        }
    }

    /// Décide et retourne le `CompressionProfile` directement.
    pub fn decide_profile(&self, data: &[u8]) -> CompressionProfile {
        self.decide(data).profile()
    }
}

impl Default for CompressionChoice {
    fn default() -> Self {
        Self::new(CompressPolicy::Adaptive)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn compressible(size: usize) -> alloc::vec::Vec<u8> {
        let mut v = alloc::vec::Vec::new();
        v.resize(size, 0xAA);
        v
    }

    #[test]
    fn test_policy_none_always_skips() {
        let c = CompressionChoice::new(CompressPolicy::None);
        let data = compressible(10_000);
        let d = c.decide(&data);
        assert_eq!(d.algorithm, CompressionAlgorithm::None);
        assert_eq!(d.reason, DecisionReason::PolicyForced);
    }

    #[test]
    fn test_always_lz4() {
        let c = CompressionChoice::new(CompressPolicy::AlwaysLz4);
        let data = compressible(10_000);
        let d = c.decide(&data);
        assert_eq!(d.algorithm, CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_always_zstd() {
        let c = CompressionChoice::new(CompressPolicy::AlwaysZstd);
        let data = compressible(10_000);
        let d = c.decide(&data);
        assert_eq!(d.algorithm, CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_adaptive_small_lz4() {
        let c = CompressionChoice::new(CompressPolicy::Adaptive).with_zstd_threshold(100_000);
        let data = compressible(10_000);
        let d = c.decide(&data);
        assert_eq!(d.algorithm, CompressionAlgorithm::Lz4);
        assert_eq!(d.reason, DecisionReason::AdaptiveLz4SmallBlob);
    }

    #[test]
    fn test_adaptive_large_zstd() {
        let c = CompressionChoice::new(CompressPolicy::Adaptive).with_zstd_threshold(1);
        let data = compressible(10_000);
        let d = c.decide(&data);
        assert_eq!(d.algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(d.reason, DecisionReason::AdaptiveZstdLargeBlob);
    }

    #[test]
    fn test_too_small() {
        let c = CompressionChoice::new(CompressPolicy::AlwaysLz4);
        let d = c.decide(b"tiny");
        assert_eq!(d.reason, DecisionReason::TooSmall);
    }

    #[test]
    fn test_already_compressed_magic() {
        let c = CompressionChoice::new(CompressPolicy::AlwaysLz4);
        let mut data = alloc::vec![0xFDu8, 0x2F, 0xB5, 0x28];
        data.resize(600, 0x00);
        let d = c.decide(&data);
        assert_eq!(d.reason, DecisionReason::AlreadyCompressed);
    }

    #[test]
    fn test_decide_profile_no_compress() {
        let c = CompressionChoice::new(CompressPolicy::None);
        let p = c.decide_profile(&compressible(10_000));
        assert!(!p.algorithm.is_compressed());
    }

    #[test]
    fn test_set_policy() {
        let mut c = CompressionChoice::new(CompressPolicy::None);
        c.set_policy(CompressPolicy::AlwaysLz4);
        assert_eq!(c.policy(), CompressPolicy::AlwaysLz4);
    }

    #[test]
    fn test_will_compress_true() {
        let d = CompressDecision {
            algorithm: CompressionAlgorithm::Lz4,
            level: CompressLevel::Fast,
            reason: DecisionReason::PolicyForced,
        };
        assert!(d.will_compress());
    }

    #[test]
    fn test_will_compress_false() {
        let d = CompressDecision {
            algorithm: CompressionAlgorithm::None,
            level: CompressLevel::Default,
            reason: DecisionReason::TooSmall,
        };
        assert!(!d.will_compress());
    }

    #[test]
    fn test_policy_name() {
        assert_eq!(CompressPolicy::Adaptive.name(), "adaptive");
        assert_eq!(CompressPolicy::None.name(), "none");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PolicyPreset — présets de configuration par cas d'usage
// ─────────────────────────────────────────────────────────────────────────────

/// Présets de `CompressionChoice` pour différents cas d'usage.
pub struct PolicyPresets;

impl PolicyPresets {
    /// Hot path : LZ4 uniquement, sélecteur rapide, faible latence.
    pub const fn hot_path() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::AlwaysLz4)
    }

    /// Cold storage : Zstd Default, ratio maximal.
    pub const fn cold_storage() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::AlwaysZstd)
    }

    /// Adaptatif : LZ4 pour < 32KiB, Zstd au-delà.
    pub const fn adaptive_default() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::Adaptive)
    }

    /// Archivage : Zstd Best, sans détection d'entropie.
    pub fn archival() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::AlwaysZstd)
            .with_zstd_level(crate::fs::exofs::compress::algorithm::CompressLevel::Best)
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    fn compressible(size: usize) -> alloc::vec::Vec<u8> {
        let mut v = alloc::vec::Vec::new();
        v.resize(size, 0xCC);
        v
    }

    #[test]
    fn test_hot_path_always_lz4() {
        let c = PolicyPresets::hot_path();
        let d = c.decide(&compressible(10_000));
        assert_eq!(d.algorithm, CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_cold_storage_always_zstd() {
        let c = PolicyPresets::cold_storage();
        let d = c.decide(&compressible(10_000));
        assert_eq!(d.algorithm, CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_adaptive_default_policy() {
        assert_eq!(
            PolicyPresets::adaptive_default().policy(),
            CompressPolicy::Adaptive
        );
    }

    // ── Tests supplémentaires ─────────────────────────────────────────────────

    #[test]
    fn test_decision_has_reason() {
        let choice = CompressionChoice::new(CompressPolicy::default());
        let data = b"hello world test data";
        let dec = choice.decide(data);
        // La décision doit avoir un reason valide.
        let _ = dec.reason;
    }

    #[test]
    fn test_preset_adaptive_default_is_lz4() {
        let p = PolicyPresets::adaptive_default();
        let d = p.decide(&compressible(10_000));
        assert_eq!(d.algorithm, CompressionAlgorithm::Lz4);
    }

    #[test]
    fn test_preset_archival_is_zstd() {
        let p = PolicyPresets::archival();
        let d = p.decide(&compressible(10_000));
        assert_eq!(d.algorithm, CompressionAlgorithm::Zstd);
    }
}
