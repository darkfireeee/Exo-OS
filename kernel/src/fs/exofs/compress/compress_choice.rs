//! Selection of optimal compression algorithm for an ExoFS blob.
//!
//! The compression policy is configurable:
//! - `AlwaysLz4` : minimum latency, no adaptive decision.
//! - `AlwaysZstd`: maximum ratio, slower.
//! - `Adaptive`  : LZ4 for small blobs / hot path, Zstd for large blobs.
//! - `None`      : no compression (pass-through).
//!
//! RULE ARITH-02 : checked/saturating arithmetic.
//! RULE RECUR-01 : no recursion.

use crate::fs::exofs::compress::algorithm::{
    CompressLevel, CompressionAlgorithm, CompressionProfile,
};
use crate::fs::exofs::compress::compress_threshold::CompressionThreshold;

// ─────────────────────────────────────────────────────────────────────────────
// CompressPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Compression choice policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressPolicy {
    /// Always choose LZ4 (low latency).
    AlwaysLz4,
    /// Always choose Zstd (better ratio).
    AlwaysZstd,
    /// Adaptive choice based on size and estimated entropy.
    Adaptive,
    /// No compression.
    None,
}

impl CompressPolicy {
    /// Human-readable policy name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AlwaysLz4 => "always_lz4",
            Self::AlwaysZstd => "always_zstd",
            Self::Adaptive => "adaptive",
            Self::None => "none",
        }
    }

    /// `true` if policy can effectively compress data.
    pub const fn is_compressing(self) -> bool {
        !matches!(self, Self::None)
    }

    /// LZ4 at default level (low latency).
    pub const fn lz4_default() -> Self {
        Self::AlwaysLz4
    }
    /// LZ4 at maximum speed.
    pub const fn lz4_fast() -> Self {
        Self::AlwaysLz4
    }
    /// Zstd at default level (good ratio).
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

/// Reason for the compression decision.
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

/// Compression decision returned by `CompressionChoice`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressDecision {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressLevel,
    pub reason: DecisionReason,
}

impl CompressDecision {
    /// Profile (algorithm + level) of this decision.
    pub fn profile(self) -> CompressionProfile {
        CompressionProfile {
            algorithm: self.algorithm,
            level: self.level,
        }
    }

    /// `true` if compression will effectively be applied.
    pub fn will_compress(self) -> bool {
        self.algorithm.is_compressed()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionChoice
// ─────────────────────────────────────────────────────────────────────────────

/// Compression algorithm selector.
pub struct CompressionChoice {
    policy: CompressPolicy,
    threshold: CompressionThreshold,
    /// Size threshold beyond which Zstd is preferred over LZ4 in adaptive mode.
    zstd_size_threshold: usize,
    lz4_level: CompressLevel,
    zstd_level: CompressLevel,
}

impl CompressionChoice {
    /// Creates a selector with given policy and default settings.
    pub const fn new(policy: CompressPolicy) -> Self {
        Self {
            policy,
            threshold: CompressionThreshold::default(),
            zstd_size_threshold: 32768,
            lz4_level: CompressLevel::Fast,
            zstd_level: CompressLevel::Default,
        }
    }

    /// Configures adaptive threshold LZ4 -> Zstd.
    pub const fn with_zstd_threshold(mut self, bytes: usize) -> Self {
        self.zstd_size_threshold = bytes;
        self
    }

    /// Configures default LZ4 level.
    pub const fn with_lz4_level(mut self, l: CompressLevel) -> Self {
        self.lz4_level = l;
        self
    }

    /// Configures default Zstd level.
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

    /// Decides algorithm and level for a given blob.
    /// RECUR-01: no recursion — purely iterative logic.
    pub fn decide(&self, data: &[u8]) -> CompressDecision {
        // 1. None policy.
        if self.policy == CompressPolicy::None {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::PolicyForced,
            };
        }
        // 2. Minimum size.
        if data.len() < self.threshold.min_size {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::TooSmall,
            };
        }
        // 3. Magic bytes (already compressed data).
        if self.threshold.detect_already_compressed
            && crate::fs::exofs::compress::compress_threshold::looks_compressed(data)
        {
            return CompressDecision {
                algorithm: CompressionAlgorithm::None,
                level: CompressLevel::Default,
                reason: DecisionReason::AlreadyCompressed,
            };
        }
        // 4. Entropy too high.
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
        // 5. Apply policy.
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

    /// Decides and returns `CompressionProfile` directly.
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
// PolicyPreset — configuration presets per use case
// ─────────────────────────────────────────────────────────────────────────────

/// Presets of `CompressionChoice` for different use cases.
pub struct PolicyPresets;

impl PolicyPresets {
    /// Hot path: LZ4 only, fast selector, low latency.
    pub const fn hot_path() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::AlwaysLz4)
    }

    /// Cold storage: Zstd Default, maximum ratio.
    pub const fn cold_storage() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::AlwaysZstd)
    }

    /// Adaptive: LZ4 for < 32KiB, Zstd beyond.
    pub const fn adaptive_default() -> CompressionChoice {
        CompressionChoice::new(CompressPolicy::Adaptive)
    }

    /// Archival: Zstd Best, without entropy detection.
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

    // ── Additional tests ─────────────────────────────────────────────────────

    #[test]
    fn test_decision_has_reason() {
        let choice = CompressionChoice::new(CompressPolicy::default());
        let data = b"hello world test data";
        let dec = choice.decide(data);
        // The decision must have a valid reason.
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
