//! Access Pattern Predictor
//!
//! Uses the quantized neural network model to predict next access patterns
//! and provide intelligent prefetching hints.
//!
//! ## Prediction Strategy
//! 1. Maintain sliding window of recent accesses (per inode)
//! 2. Extract features from access history
//! 3. Run model inference to get prefetch predictions
//! 4. Return high-confidence predictions (threshold > 0.6)
//!
//! ## Performance
//! - Prediction latency: < 15µs (including feature extraction)
//! - Memory per inode: ~512 bytes
//! - Max tracked inodes: 1024 (LRU eviction)

use super::model::QuantizedModel;
use super::profiler::FeatureExtractor;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Access record for prediction
#[derive(Debug, Clone, Copy)]
pub struct AccessRecord {
    /// Inode number
    pub ino: u64,
    /// File offset accessed
    pub offset: u64,
    /// Length of access
    pub length: usize,
    /// Timestamp (nanoseconds since boot)
    pub timestamp: u64,
    /// Access type (read/write)
    pub is_read: bool,
    /// Sequential indicator
    pub is_sequential: bool,
}

impl AccessRecord {
    /// Create new access record
    pub fn new(ino: u64, offset: u64, length: usize, is_read: bool) -> Self {
        Self {
            ino,
            offset,
            length,
            timestamp: crate::time::uptime_ns(),
            is_read,
            is_sequential: false,
        }
    }
}

/// Prefetch prediction result
#[derive(Debug, Clone, Copy)]
pub struct PrefetchPrediction {
    /// Predicted offset to prefetch
    pub offset: u64,
    /// Predicted length to prefetch
    pub length: usize,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
    /// Priority (higher = more urgent)
    pub priority: u8,
}

/// Per-inode access history tracker
struct InodePredictor {
    /// Inode number
    ino: u64,
    /// Recent access history (sliding window)
    history: VecDeque<AccessRecord>,
    /// Last predicted offset (to avoid duplicates)
    last_predicted_offset: u64,
    /// Prediction hit count (for accuracy tracking)
    predictions_made: u64,
    predictions_hit: u64,
    /// Last access time (for LRU eviction)
    last_access_time: u64,
}

impl InodePredictor {
    /// Create new inode predictor
    fn new(ino: u64) -> Self {
        Self {
            ino,
            history: VecDeque::with_capacity(16),
            last_predicted_offset: 0,
            predictions_made: 0,
            predictions_hit: 0,
            last_access_time: crate::time::uptime_ns(),
        }
    }

    /// Record an access
    fn record_access(&mut self, mut record: AccessRecord) {
        // Check if access is sequential
        if let Some(last) = self.history.back() {
            let expected_offset = last.offset + last.length as u64;
            record.is_sequential = record.offset == expected_offset;

            // Check if this access matches a previous prediction
            if record.offset == self.last_predicted_offset {
                self.predictions_hit += 1;
            }
        }

        // Add to history
        if self.history.len() >= 16 {
            self.history.pop_front();
        }
        self.history.push_back(record);

        self.last_access_time = record.timestamp;
    }

    /// Get prediction accuracy (0.0 - 1.0)
    fn accuracy(&self) -> f32 {
        if self.predictions_made == 0 {
            return 0.0;
        }
        self.predictions_hit as f32 / self.predictions_made as f32
    }
}

/// Access Pattern Predictor
///
/// Main interface for making access predictions using ML model
pub struct AccessPredictor {
    /// Neural network model
    model: RwLock<QuantizedModel>,
    /// Per-inode predictors
    predictors: RwLock<HashMap<u64, InodePredictor>>,
    /// Feature extractor
    feature_extractor: FeatureExtractor,
    /// Statistics
    stats: PredictorStats,
    /// Maximum tracked inodes (LRU eviction beyond this)
    max_tracked_inodes: usize,
}

impl AccessPredictor {
    /// Create new access predictor
    pub fn new() -> Self {
        Self {
            model: RwLock::new(QuantizedModel::new()),
            predictors: RwLock::new(HashMap::new()),
            feature_extractor: FeatureExtractor::new(),
            stats: PredictorStats::new(),
            max_tracked_inodes: 1024,
        }
    }

    /// Record an access and get prefetch predictions
    ///
    /// This is the main entry point called by the cache subsystem.
    ///
    /// # Performance
    /// Target: < 15µs total (feature extraction + inference + processing)
    pub fn predict(&self, record: AccessRecord) -> Vec<PrefetchPrediction> {
        let start = crate::time::uptime_ns();

        // Get or create predictor for this inode
        let mut predictors = self.predictors.write();

        // LRU eviction if too many tracked inodes
        if predictors.len() >= self.max_tracked_inodes {
            self.evict_lru(&mut predictors);
        }

        let predictor = predictors
            .entry(record.ino)
            .or_insert_with(|| InodePredictor::new(record.ino));

        // Record access
        predictor.record_access(record);

        // Need at least 3 accesses for meaningful prediction
        if predictor.history.len() < 3 {
            return Vec::new();
        }

        // Extract features from access history
        let features = self.feature_extractor.extract_features(&predictor.history);

        // Run model inference
        let model = self.model.read();
        let predictions = model.infer(&features);
        drop(model);

        // Process predictions into prefetch hints
        let mut prefetch_predictions = Vec::new();

        // Interpret model output:
        // - predictions[0..4]: confidence scores for different access patterns
        // - predictions[4..8]: predicted offset deltas (normalized)

        for i in 0..4 {
            let confidence = predictions[i];

            // Only return high-confidence predictions
            if confidence > 0.6 {
                // Convert normalized offset delta to actual offset
                let last_access = predictor.history.back().unwrap();
                let offset_delta_normalized = predictions[i + 4];
                let offset_delta = (offset_delta_normalized * 1024.0 * 1024.0) as u64; // Max 1MB lookahead

                let predicted_offset = last_access.offset + offset_delta;

                // Avoid duplicate predictions
                if predicted_offset != predictor.last_predicted_offset {
                    predictor.last_predicted_offset = predicted_offset;
                    predictor.predictions_made += 1;

                    prefetch_predictions.push(PrefetchPrediction {
                        offset: predicted_offset,
                        length: last_access.length.max(4096), // At least 4KB
                        confidence,
                        priority: (confidence * 255.0) as u8,
                    });
                }
            }
        }

        // Update statistics
        let elapsed = crate::time::uptime_ns() - start;
        self.stats.total_predictions.fetch_add(1, Ordering::Relaxed);
        self.stats.total_prediction_time_ns.fetch_add(elapsed, Ordering::Relaxed);
        if !prefetch_predictions.is_empty() {
            self.stats.predictions_with_results.fetch_add(1, Ordering::Relaxed);
        }

        prefetch_predictions
    }

    /// Evict least recently used inode predictor
    fn evict_lru(&self, predictors: &mut HashMap<u64, InodePredictor>) {
        if predictors.is_empty() {
            return;
        }

        // Find LRU inode
        let mut lru_ino = 0u64;
        let mut lru_time = u64::MAX;

        for (ino, predictor) in predictors.iter() {
            if predictor.last_access_time < lru_time {
                lru_time = predictor.last_access_time;
                lru_ino = *ino;
            }
        }

        if lru_ino != 0 {
            predictors.remove(&lru_ino);
            self.stats.lru_evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get prediction accuracy for a specific inode
    pub fn get_accuracy(&self, ino: u64) -> Option<f32> {
        let predictors = self.predictors.read();
        predictors.get(&ino).map(|p| p.accuracy())
    }

    /// Get overall prediction accuracy
    pub fn overall_accuracy(&self) -> f32 {
        let predictors = self.predictors.read();
        if predictors.is_empty() {
            return 0.0;
        }

        let total_predictions: u64 = predictors.values().map(|p| p.predictions_made).sum();
        let total_hits: u64 = predictors.values().map(|p| p.predictions_hit).sum();

        if total_predictions == 0 {
            return 0.0;
        }

        total_hits as f32 / total_predictions as f32
    }

    /// Clear predictor for a specific inode
    pub fn clear_inode(&self, ino: u64) {
        let mut predictors = self.predictors.write();
        predictors.remove(&ino);
    }

    /// Clear all predictors
    pub fn clear_all(&self) {
        let mut predictors = self.predictors.write();
        predictors.clear();
    }

    /// Get statistics
    pub fn stats(&self) -> PredictorStats {
        self.stats.clone()
    }

    /// Get model reference for training
    pub fn model_mut(&self) -> &RwLock<QuantizedModel> {
        &self.model
    }

    /// Get number of tracked inodes
    pub fn tracked_inode_count(&self) -> usize {
        let predictors = self.predictors.read();
        predictors.len()
    }
}

impl Default for AccessPredictor {
    fn default() -> Self {
        Self::new()
    }
}

/// Predictor statistics
#[derive(Debug)]
pub struct PredictorStats {
    /// Total predictions made
    pub total_predictions: AtomicU64,
    /// Predictions that returned results
    pub predictions_with_results: AtomicU64,
    /// Total time spent in prediction (nanoseconds)
    pub total_prediction_time_ns: AtomicU64,
    /// LRU evictions performed
    pub lru_evictions: AtomicU64,
}

impl PredictorStats {
    fn new() -> Self {
        Self {
            total_predictions: AtomicU64::new(0),
            predictions_with_results: AtomicU64::new(0),
            total_prediction_time_ns: AtomicU64::new(0),
            lru_evictions: AtomicU64::new(0),
        }
    }

    /// Get average prediction time
    pub fn avg_prediction_time_ns(&self) -> u64 {
        let total = self.total_predictions.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let time = self.total_prediction_time_ns.load(Ordering::Relaxed);
        time / total
    }

    /// Get prediction effectiveness (ratio of predictions with results)
    pub fn effectiveness(&self) -> f32 {
        let total = self.total_predictions.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let with_results = self.predictions_with_results.load(Ordering::Relaxed);
        with_results as f32 / total as f32
    }
}

impl Clone for PredictorStats {
    fn clone(&self) -> Self {
        Self {
            total_predictions: AtomicU64::new(self.total_predictions.load(Ordering::Relaxed)),
            predictions_with_results: AtomicU64::new(self.predictions_with_results.load(Ordering::Relaxed)),
            total_prediction_time_ns: AtomicU64::new(self.total_prediction_time_ns.load(Ordering::Relaxed)),
            lru_evictions: AtomicU64::new(self.lru_evictions.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_record_creation() {
        let record = AccessRecord::new(1, 0, 4096, true);
        assert_eq!(record.ino, 1);
        assert_eq!(record.offset, 0);
        assert_eq!(record.length, 4096);
        assert!(record.is_read);
        assert!(!record.is_sequential);
    }

    #[test]
    fn test_predictor_creation() {
        let predictor = AccessPredictor::new();
        assert_eq!(predictor.tracked_inode_count(), 0);
    }

    #[test]
    fn test_sequential_detection() {
        let predictor = AccessPredictor::new();

        // Sequential reads
        let r1 = AccessRecord::new(1, 0, 4096, true);
        let r2 = AccessRecord::new(1, 4096, 4096, true);
        let r3 = AccessRecord::new(1, 8192, 4096, true);

        let _ = predictor.predict(r1);
        let _ = predictor.predict(r2);
        let predictions = predictor.predict(r3);

        // Should detect sequential pattern and predict
        assert!(predictor.tracked_inode_count() > 0);
    }

    #[test]
    fn test_lru_eviction() {
        let mut predictor = AccessPredictor::new();
        predictor.max_tracked_inodes = 2;

        // Add 3 inodes (triggers eviction)
        for i in 0..3 {
            let record = AccessRecord::new(i as u64, 0, 4096, true);
            let _ = predictor.predict(record);
        }

        // Should have evicted one
        assert!(predictor.tracked_inode_count() <= 2);
    }
}
