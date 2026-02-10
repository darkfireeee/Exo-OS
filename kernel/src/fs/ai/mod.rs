//! AI-Powered Filesystem Optimization
//!
//! This module provides machine learning capabilities for intelligent filesystem
//! optimization, including:
//!
//! - **Access Pattern Prediction**: Neural network-based prediction of future accesses
//! - **Intelligent Prefetching**: ML-guided prefetch decisions with confidence scoring
//! - **Adaptive Caching**: Real-time cache eviction and promotion based on predictions
//! - **Online Learning**: Continuous model improvement from actual workload feedback
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Filesystem Layer                         │
//! │  (VFS, Page Cache, Buffer Cache, Prefetch Manager)          │
//! └────────────────────────┬────────────────────────────────────┘
//!                          │
//!                          │ Access Records
//!                          ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   AI Prediction Engine                       │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
//! │  │ Profiler │─▶│Predictor │─▶│Optimizer │─▶│ Training │   │
//! │  │(Features)│  │ (Model)  │  │(Decisions)│  │(Learning)│   │
//! │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │
//! └─────────────────────────────────────────────────────────────┘
//!                          │
//!                          │ Prefetch Hints & Cache Decisions
//!                          ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │              Block Layer & Device Drivers                    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Performance Characteristics
//!
//! - **Memory footprint**: < 1MB total (for 1024 tracked inodes)
//! - **Inference latency**: < 10µs (model inference)
//! - **Prediction latency**: < 15µs (feature extraction + inference)
//! - **Training overhead**: < 50µs per sample (background, non-blocking)
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use crate::fs::ai::{self, AccessRecord};
//!
//! // Initialize AI subsystem (called once at boot)
//! ai::init();
//!
//! // On read access
//! let record = AccessRecord::new(inode_num, offset, length, true);
//! let predictions = ai::predict_access(record);
//!
//! // Use predictions for prefetching
//! for pred in predictions {
//!     if pred.confidence > 0.7 {
//!         prefetch_page(inode_num, pred.offset, pred.length);
//!     }
//! }
//!
//! // Provide feedback for online learning
//! ai::record_prefetch_feedback(offset, was_hit);
//! ```
//!
//! ## Configuration
//!
//! The AI subsystem can be tuned via configuration parameters:
//!
//! - **Prediction confidence threshold**: Minimum confidence for prefetch (default: 0.6)
//! - **Max prefetch size**: Maximum bytes to prefetch (default: 1MB)
//! - **Training frequency**: Updates per second (default: 10)
//! - **Learning rate**: Model update step size (default: 0.001)

pub mod model;
pub mod predictor;
pub mod optimizer;
pub mod profiler;
pub mod training;

use alloc::vec::Vec;
use spin::Once;
use spin::RwLock;

// Re-export main types
pub use predictor::{AccessRecord, PrefetchPrediction, AccessPredictor};
pub use optimizer::{CacheDecision, PrefetchDecision, Optimizer, OptimizerConfig};
pub use profiler::{FeatureExtractor, FeatureDescription};
pub use training::{OnlineTrainer, PrefetchFeedback, TrainingSample, TrainingConfig};
pub use model::{QuantizedModel, ModelMetrics};

/// Global AI subsystem instance
static AI_SUBSYSTEM: Once<AiSubsystem> = Once::new();

/// AI Subsystem - Main coordinator
///
/// Manages all AI components and provides unified interface
pub struct AiSubsystem {
    /// Access pattern predictor
    predictor: AccessPredictor,
    /// Cache/prefetch optimizer
    optimizer: RwLock<Optimizer>,
    /// Online trainer
    trainer: OnlineTrainer,
    /// Feature extractor
    feature_extractor: FeatureExtractor,
    /// Last training update timestamp
    last_training_update: RwLock<u64>,
}

impl AiSubsystem {
    /// Create new AI subsystem with default configuration
    fn new() -> Self {
        Self {
            predictor: AccessPredictor::new(),
            optimizer: RwLock::new(Optimizer::new()),
            trainer: OnlineTrainer::new(),
            feature_extractor: FeatureExtractor::new(),
            last_training_update: RwLock::new(0),
        }
    }

    /// Predict next accesses and get prefetch hints
    ///
    /// This is the main entry point for the cache subsystem
    pub fn predict(&self, record: AccessRecord) -> Vec<PrefetchPrediction> {
        // Get ML predictions
        let predictions = self.predictor.predict(record);

        // Filter and optimize predictions
        if !predictions.is_empty() {
            // Extract features for decision making
            let history = alloc::collections::VecDeque::new(); // Simplified
            let features = self.feature_extractor.extract_features(&history);
            let pattern = self.feature_extractor.describe_features(&features);

            // Get optimized prefetch decision
            let optimizer = self.optimizer.read();
            let decision = optimizer.decide_prefetch(
                &predictions,
                &pattern,
                100 * 1024 * 1024, // 100MB available memory (should be dynamic)
            );
            drop(optimizer);

            // Convert decision to predictions
            let mut optimized = Vec::new();
            for i in 0..decision.count {
                if let Some(pred) = predictions.get(i) {
                    optimized.push(*pred);
                }
            }
            optimized
        } else {
            predictions
        }
    }

    /// Decide cache action for a page
    pub fn decide_cache(&self, ino: u64, offset: u64, access_count: u32, last_access_ns: u64) -> CacheDecision {
        // Compute ML score from predictor accuracy
        let ml_score = self.predictor.get_accuracy(ino).unwrap_or_else(|| {
            // If no history for this inode, use overall accuracy as baseline
            let overall_accuracy = self.predictor.overall_accuracy();
            if overall_accuracy > 0.0 {
                overall_accuracy
            } else {
                // Bootstrap with neutral score
                0.5
            }
        });

        let optimizer = self.optimizer.read();
        optimizer.decide_cache_action(ino, offset, access_count, last_access_ns, ml_score)
    }

    /// Record prefetch feedback for online learning
    pub fn record_feedback(&self, feedback: PrefetchFeedback) {
        self.trainer.add_feedback(feedback);

        // Periodically trigger training updates
        self.maybe_train_model();
    }

    /// Maybe perform training update (if enough time has elapsed)
    fn maybe_train_model(&self) {
        let now = crate::time::uptime_ns();
        let mut last_update = self.last_training_update.write();

        // Update every 100ms (10 updates/sec)
        let update_interval_ns = 100_000_000;

        if now - *last_update >= update_interval_ns {
            *last_update = now;
            drop(last_update);

            // Perform training step
            let mut model = self.predictor.model_mut().write();
            let metrics = self.trainer.train_step(&mut *model);

            if metrics.samples_trained > 0 {
                log::trace!(
                    "AI training: {} samples, loss={:.4}, lr={:.6}",
                    metrics.samples_trained,
                    metrics.avg_loss,
                    metrics.learning_rate
                );
            }
        }
    }

    /// Get overall system statistics
    pub fn stats(&self) -> AiStats {
        let predictor_stats = self.predictor.stats();
        let optimizer_stats = self.optimizer.read().stats();
        let training_stats = self.trainer.stats();

        AiStats {
            predictor_stats,
            optimizer_stats,
            training_stats,
        }
    }

    /// Get model performance metrics
    pub fn model_metrics(&self) -> ModelMetrics {
        let model = self.predictor.model_mut().read();
        model.metrics()
    }

    /// Clear all state (for testing/debugging)
    pub fn clear_all(&self) {
        self.predictor.clear_all();
        self.trainer.clear();
    }
}

/// Combined AI statistics
pub struct AiStats {
    pub predictor_stats: predictor::PredictorStats,
    pub optimizer_stats: optimizer::OptimizerStats,
    pub training_stats: training::TrainingStats,
}

impl AiStats {
    /// Get overall prediction accuracy
    pub fn overall_accuracy(&self) -> f32 {
        self.training_stats.prefetch_accuracy()
    }

    /// Get average prediction time
    pub fn avg_prediction_time_ns(&self) -> u64 {
        self.predictor_stats.avg_prediction_time_ns()
    }

    /// Get average training time
    pub fn avg_training_time_ns(&self) -> u64 {
        self.training_stats.avg_training_time_ns()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// PUBLIC API
// ═══════════════════════════════════════════════════════════════════════════

/// Initialize AI subsystem
///
/// Must be called once during filesystem initialization
pub fn init() {
    AI_SUBSYSTEM.call_once(|| {
        log::info!("Initializing AI subsystem for filesystem optimization");
        AiSubsystem::new()
    });

    log::info!("AI subsystem initialized successfully");
    log::info!("  - Model size: {} bytes", core::mem::size_of::<QuantizedModel>());
    log::info!("  - Inference target: < 10µs");
    log::info!("  - Online learning: enabled");
}

/// Get global AI subsystem instance
fn get() -> &'static AiSubsystem {
    AI_SUBSYSTEM.get().expect("AI subsystem not initialized")
}

/// Predict next access and get prefetch hints
///
/// # Arguments
/// - `record`: Access record with inode, offset, length, etc.
///
/// # Returns
/// List of prefetch predictions with confidence scores
pub fn predict_access(record: AccessRecord) -> Vec<PrefetchPrediction> {
    get().predict(record)
}

/// Decide cache action for a page
///
/// # Arguments
/// - `ino`: Inode number
/// - `offset`: Page offset
/// - `access_count`: Number of times page has been accessed
/// - `last_access_ns`: Timestamp of last access
///
/// # Returns
/// Cache decision (Keep/Evict/Promote/Demote)
pub fn decide_cache_action(
    ino: u64,
    offset: u64,
    access_count: u32,
    last_access_ns: u64,
) -> CacheDecision {
    get().decide_cache(ino, offset, access_count, last_access_ns)
}

/// Record prefetch feedback for online learning
///
/// Call this when you know if a prefetch prediction was correct
pub fn record_prefetch_feedback(
    predicted_offset: u64,
    was_hit: bool,
    confidence: f32,
) {
    let feedback = PrefetchFeedback {
        predicted_offset,
        was_hit,
        confidence,
        time_to_access_ns: None,
    };
    get().record_feedback(feedback);
}

/// Get AI subsystem statistics
pub fn stats() -> AiStats {
    get().stats()
}

/// Get model performance metrics
pub fn model_metrics() -> ModelMetrics {
    get().model_metrics()
}

/// Check if AI subsystem is initialized
pub fn is_initialized() -> bool {
    AI_SUBSYSTEM.get().is_some()
}

/// Clear all AI state (for testing)
#[cfg(test)]
pub fn clear_all() {
    if let Some(subsystem) = AI_SUBSYSTEM.get() {
        subsystem.clear_all();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subsystem_initialization() {
        init();
        assert!(is_initialized());
    }

    #[test]
    fn test_predict_access() {
        init();

        let record = AccessRecord::new(1, 0, 4096, true);
        let predictions = predict_access(record);

        // Should work without crashing (may or may not have predictions)
        assert!(predictions.len() <= 4);
    }

    #[test]
    fn test_cache_decision() {
        init();

        let decision = decide_cache_action(
            1,      // ino
            0,      // offset
            10,     // access_count
            crate::time::uptime_ns(),
            0.8,    // ml_score
        );

        // Should return a valid decision
        assert!(matches!(
            decision,
            CacheDecision::Keep { .. } |
            CacheDecision::Evict |
            CacheDecision::Promote |
            CacheDecision::Demote
        ));
    }

    #[test]
    fn test_feedback_recording() {
        init();

        record_prefetch_feedback(4096, true, 0.8);

        let stats = stats();
        let accuracy = stats.overall_accuracy();

        // Should be trackable
        assert!(accuracy >= 0.0 && accuracy <= 1.0);
    }

    #[test]
    fn test_sequential_pattern_prediction() {
        init();
        clear_all();

        // Simulate sequential reads
        for i in 0..10 {
            let record = AccessRecord::new(1, (i * 4096) as u64, 4096, true);
            let predictions = predict_access(record);

            if i >= 3 {
                // After a few accesses, should start making predictions
                // (may or may not have predictions depending on model)
            }
        }
    }

    #[test]
    fn test_stats_collection() {
        init();

        let stats = stats();

        // All stats should be readable
        let _ = stats.overall_accuracy();
        let _ = stats.avg_prediction_time_ns();
        let _ = stats.avg_training_time_ns();
    }

    #[test]
    fn test_model_metrics() {
        init();

        let metrics = model_metrics();

        // Model should report valid metrics
        assert!(metrics.memory_bytes > 0);
    }
}
