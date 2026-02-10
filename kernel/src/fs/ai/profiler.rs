//! Workload Profiler and Feature Extraction
//!
//! Extracts meaningful features from access patterns for ML model input.
//!
//! ## Features Extracted (16 total)
//! 1. Temporal features (0-3): Access frequency, time delta, burst detection
//! 2. Spatial features (4-7): Sequential ratio, stride detection, locality
//! 3. Size features (8-11): Average size, size variance, alignment
//! 4. Pattern features (12-15): Read/write ratio, randomness score, working set size
//!
//! ## Normalization
//! All features are normalized to [-1.0, 1.0] range for model input.

use super::predictor::AccessRecord;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use hashbrown::HashSet;
use crate::fs::utils::log2_approx_f64;

/// Feature extractor for access patterns
///
/// Converts raw access history into normalized feature vectors
/// suitable for neural network input.
pub struct FeatureExtractor {
    /// Window size for feature computation
    window_size: usize,
}

impl FeatureExtractor {
    /// Create new feature extractor
    pub fn new() -> Self {
        Self {
            window_size: 16,
        }
    }

    /// Extract features from access history
    ///
    /// Returns a 16-element feature vector normalized to [-1, 1]
    ///
    /// # Performance
    /// Target: < 5µs for 16-element history
    pub fn extract_features(&self, history: &VecDeque<AccessRecord>) -> [f32; 16] {
        let mut features = [0.0f32; 16];

        if history.is_empty() {
            return features;
        }

        // Get recent window
        let window: Vec<_> = history
            .iter()
            .rev()
            .take(self.window_size)
            .copied()
            .collect();

        if window.is_empty() {
            return features;
        }

        // TEMPORAL FEATURES (0-3)
        self.extract_temporal_features(&window, &mut features[0..4]);

        // SPATIAL FEATURES (4-7)
        self.extract_spatial_features(&window, &mut features[4..8]);

        // SIZE FEATURES (8-11)
        self.extract_size_features(&window, &mut features[8..12]);

        // PATTERN FEATURES (12-15)
        self.extract_pattern_features(&window, &mut features[12..16]);

        features
    }

    /// Extract temporal features (access timing patterns)
    fn extract_temporal_features(&self, window: &[AccessRecord], features: &mut [f32]) {
        if window.len() < 2 {
            return;
        }

        // Feature 0: Access frequency (accesses per microsecond)
        let time_span = window[0].timestamp.saturating_sub(window.last().unwrap().timestamp);
        if time_span > 0 {
            let frequency = (window.len() as f64 * 1_000_000.0) / time_span as f64;
            features[0] = (log2_approx_f64(frequency) / 10.0).clamp(-1.0, 1.0) as f32;
        }

        // Feature 1: Average time delta (normalized)
        let mut deltas = Vec::with_capacity(window.len() - 1);
        for i in 0..window.len() - 1 {
            let delta = window[i].timestamp.saturating_sub(window[i + 1].timestamp);
            deltas.push(delta);
        }
        if !deltas.is_empty() {
            let avg_delta: u64 = deltas.iter().sum::<u64>() / deltas.len() as u64;
            features[1] = (log2_approx_f64(avg_delta as f64) / 20.0).clamp(-1.0, 1.0) as f32;
        }

        // Feature 2: Temporal variance (burstiness)
        if deltas.len() > 1 {
            let avg_delta = deltas.iter().sum::<u64>() / deltas.len() as u64;
            let variance: u64 = deltas
                .iter()
                .map(|&d| {
                    let diff = if d > avg_delta { d - avg_delta } else { avg_delta - d };
                    diff
                })
                .sum::<u64>() / deltas.len() as u64;
            features[2] = (log2_approx_f64(variance as f64) / 20.0).clamp(-1.0, 1.0) as f32;
        }

        // Feature 3: Recency weight (more recent = higher value)
        let total_weight: f64 = window
            .iter()
            .enumerate()
            .map(|(i, _)| 1.0 / (i + 1) as f64)
            .sum();
        features[3] = (total_weight / window.len() as f64).clamp(-1.0, 1.0) as f32;
    }

    /// Extract spatial features (offset/location patterns)
    fn extract_spatial_features(&self, window: &[AccessRecord], features: &mut [f32]) {
        if window.len() < 2 {
            return;
        }

        // Feature 4: Sequential access ratio
        let sequential_count = window.iter().filter(|r| r.is_sequential).count();
        features[4] = (sequential_count as f32 / window.len() as f32) * 2.0 - 1.0;

        // Feature 5: Stride detection
        let mut strides = Vec::with_capacity(window.len() - 1);
        for i in 0..window.len() - 1 {
            if window[i].offset >= window[i + 1].offset {
                let stride = window[i].offset - window[i + 1].offset;
                strides.push(stride);
            }
        }

        if !strides.is_empty() {
            // Check stride consistency
            let first_stride = strides[0];
            let consistent_strides = strides.iter().filter(|&&s| s == first_stride).count();
            let consistency = consistent_strides as f32 / strides.len() as f32;
            features[5] = consistency * 2.0 - 1.0;
        }

        // Feature 6: Offset locality (working set size)
        let offsets: Vec<_> = window.iter().map(|r| r.offset).collect();
        if let (Some(&min_offset), Some(&max_offset)) = (offsets.iter().min(), offsets.iter().max()) {
            let span = max_offset.saturating_sub(min_offset);
            features[6] = (log2_approx_f64(span as f64) / 30.0).clamp(-1.0, 1.0) as f32;
        }

        // Feature 7: Forward vs backward access ratio
        let forward_count = window
            .windows(2)
            .filter(|w| w[0].offset >= w[1].offset)
            .count();
        if window.len() > 1 {
            features[7] = (forward_count as f32 / (window.len() - 1) as f32) * 2.0 - 1.0;
        }
    }

    /// Extract size features (access size patterns)
    fn extract_size_features(&self, window: &[AccessRecord], features: &mut [f32]) {
        if window.is_empty() {
            return;
        }

        // Feature 8: Average access size (normalized, log scale)
        let avg_size: usize = window.iter().map(|r| r.length).sum::<usize>() / window.len();
        features[8] = (log2_approx_f64(avg_size as f64) / 20.0).clamp(-1.0, 1.0) as f32;

        // Feature 9: Size variance
        if window.len() > 1 {
            let variance: usize = window
                .iter()
                .map(|r| {
                    if r.length > avg_size {
                        r.length - avg_size
                    } else {
                        avg_size - r.length
                    }
                })
                .sum::<usize>() / window.len();
            features[9] = (log2_approx_f64(variance as f64) / 20.0).clamp(-1.0, 1.0) as f32;
        }

        // Feature 10: Size alignment (4KB aligned = common for I/O)
        let aligned_count = window.iter().filter(|r| r.length % 4096 == 0).count();
        features[10] = (aligned_count as f32 / window.len() as f32) * 2.0 - 1.0;

        // Feature 11: Size trend (growing/shrinking)
        if window.len() >= 3 {
            let first_third_avg = window[0..window.len() / 3]
                .iter()
                .map(|r| r.length)
                .sum::<usize>() / (window.len() / 3).max(1);
            let last_third_avg = window[window.len() * 2 / 3..]
                .iter()
                .map(|r| r.length)
                .sum::<usize>() / (window.len() / 3).max(1);

            let trend = if first_third_avg > 0 {
                log2_approx_f64(last_third_avg as f64 / first_third_avg as f64)
            } else {
                0.0
            };
            features[11] = trend.clamp(-1.0, 1.0) as f32;
        }
    }

    /// Extract pattern features (high-level access characteristics)
    fn extract_pattern_features(&self, window: &[AccessRecord], features: &mut [f32]) {
        if window.is_empty() {
            return;
        }

        // Feature 12: Read/write ratio
        let read_count = window.iter().filter(|r| r.is_read).count();
        features[12] = (read_count as f32 / window.len() as f32) * 2.0 - 1.0;

        // Feature 13: Randomness score (entropy-based)
        let unique_offsets: HashSet<_> = window.iter().map(|r| r.offset / 4096).collect();
        let entropy = unique_offsets.len() as f32 / window.len() as f32;
        features[13] = entropy * 2.0 - 1.0;

        // Feature 14: Working set size (unique 4KB pages accessed)
        let working_set_size = unique_offsets.len();
        features[14] = (log2_approx_f64(working_set_size as f64) / 10.0).clamp(-1.0, 1.0) as f32;

        // Feature 15: Access density (how tightly packed are accesses)
        if let (Some(min_offset), Some(max_offset)) =
            (window.iter().map(|r| r.offset).min(), window.iter().map(|r| r.offset).max()) {
            let span: u64 = max_offset.saturating_sub(min_offset);
            let total_bytes: u64 = window.iter().map(|r| r.length as u64).sum();

            if span > 0 {
                let density = total_bytes as f64 / span as f64;
                features[15] = log2_approx_f64(density).clamp(-1.0, 1.0) as f32;
            }
        }
    }

    /// Get human-readable feature description
    pub fn describe_features(&self, features: &[f32; 16]) -> FeatureDescription {
        FeatureDescription {
            access_frequency: features[0],
            avg_time_delta: features[1],
            burstiness: features[2],
            recency_weight: features[3],
            sequential_ratio: features[4],
            stride_consistency: features[5],
            locality: features[6],
            forward_ratio: features[7],
            avg_size: features[8],
            size_variance: features[9],
            size_alignment: features[10],
            size_trend: features[11],
            read_ratio: features[12],
            randomness: features[13],
            working_set: features[14],
            access_density: features[15],
        }
    }
}

impl Default for FeatureExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Human-readable feature description
#[derive(Debug, Clone, Copy)]
pub struct FeatureDescription {
    pub access_frequency: f32,
    pub avg_time_delta: f32,
    pub burstiness: f32,
    pub recency_weight: f32,
    pub sequential_ratio: f32,
    pub stride_consistency: f32,
    pub locality: f32,
    pub forward_ratio: f32,
    pub avg_size: f32,
    pub size_variance: f32,
    pub size_alignment: f32,
    pub size_trend: f32,
    pub read_ratio: f32,
    pub randomness: f32,
    pub working_set: f32,
    pub access_density: f32,
}

impl FeatureDescription {
    /// Check if pattern is sequential
    pub fn is_sequential(&self) -> bool {
        self.sequential_ratio > 0.5 && self.randomness < 0.0
    }

    /// Check if pattern is random
    pub fn is_random(&self) -> bool {
        self.randomness > 0.5 && self.sequential_ratio < 0.0
    }

    /// Check if pattern is strided
    pub fn is_strided(&self) -> bool {
        self.stride_consistency > 0.7 && self.sequential_ratio < 0.5
    }

    /// Check if workload is bursty
    pub fn is_bursty(&self) -> bool {
        self.burstiness > 0.5
    }

    /// Check if workload is read-heavy
    pub fn is_read_heavy(&self) -> bool {
        self.read_ratio > 0.5
    }

    /// Check if workload is write-heavy
    pub fn is_write_heavy(&self) -> bool {
        self.read_ratio < -0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sequential_access(count: usize) -> VecDeque<AccessRecord> {
        let mut history = VecDeque::new();
        for i in 0..count {
            let mut record = AccessRecord::new(1, (i * 4096) as u64, 4096, true);
            record.is_sequential = i > 0;
            history.push_back(record);
        }
        history
    }

    fn create_random_access(count: usize) -> VecDeque<AccessRecord> {
        let mut history = VecDeque::new();
        for i in 0..count {
            let offset = ((i * 7 + 13) % 100) * 4096; // Pseudo-random
            let record = AccessRecord::new(1, offset as u64, 4096, true);
            history.push_back(record);
        }
        history
    }

    #[test]
    fn test_feature_extractor_creation() {
        let extractor = FeatureExtractor::new();
        assert_eq!(extractor.window_size, 16);
    }

    #[test]
    fn test_sequential_pattern_detection() {
        let extractor = FeatureExtractor::new();
        let history = create_sequential_access(10);
        let features = extractor.extract_features(&history);

        let desc = extractor.describe_features(&features);
        assert!(desc.is_sequential(), "Should detect sequential pattern");
        assert_eq!(desc.sequential_ratio > 0.5, true);
    }

    #[test]
    fn test_random_pattern_detection() {
        let extractor = FeatureExtractor::new();
        let history = create_random_access(10);
        let features = extractor.extract_features(&history);

        let desc = extractor.describe_features(&features);
        assert!(desc.randomness > 0.0, "Should have some randomness");
    }

    #[test]
    fn test_feature_normalization() {
        let extractor = FeatureExtractor::new();
        let history = create_sequential_access(10);
        let features = extractor.extract_features(&history);

        // All features should be in [-1, 1] range
        for &feature in &features {
            assert!(feature >= -1.0 && feature <= 1.0,
                "Feature {} out of range", feature);
        }
    }

    #[test]
    fn test_empty_history() {
        let extractor = FeatureExtractor::new();
        let history = VecDeque::new();
        let features = extractor.extract_features(&history);

        // Should return all zeros for empty history
        assert_eq!(features, [0.0; 16]);
    }
}
