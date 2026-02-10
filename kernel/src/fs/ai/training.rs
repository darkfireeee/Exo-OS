//! Online Learning and Model Updates
//!
//! Implements continuous model improvement through online learning:
//! - Exponential moving averages for weight updates
//! - Lightweight gradient descent (no full backpropagation)
//! - Feedback from prefetch accuracy
//! - Adaptive learning rate based on prediction accuracy
//!
//! ## Training Strategy
//! 1. Collect feedback from cache hits/misses
//! 2. Compute simple gradients (reward/penalty signals)
//! 3. Update model weights incrementally
//! 4. Adjust learning rate based on performance
//!
//! ## Performance
//! - Training updates: < 50µs per sample
//! - No blocking (updates run in background if needed)
//! - Memory overhead: < 128 bytes per training sample

use super::model::QuantizedModel;
use super::predictor::{AccessRecord, PrefetchPrediction};
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use crate::fs::utils::powi_approx;

/// Training sample for model updates
#[derive(Debug, Clone, Copy)]
pub struct TrainingSample {
    /// Input features
    pub features: [f32; 16],
    /// Target output (ground truth)
    pub target: [f32; 8],
    /// Sample weight (importance)
    pub weight: f32,
    /// Timestamp
    pub timestamp: u64,
}

/// Feedback from prefetch operation
#[derive(Debug, Clone, Copy)]
pub struct PrefetchFeedback {
    /// Predicted offset
    pub predicted_offset: u64,
    /// Was prediction correct (page accessed within window)?
    pub was_hit: bool,
    /// Confidence of prediction
    pub confidence: f32,
    /// Time until actual access (if hit)
    pub time_to_access_ns: Option<u64>,
}

/// Online trainer for the ML model
pub struct OnlineTrainer {
    /// Training sample buffer (ring buffer)
    sample_buffer: RwLock<VecDeque<TrainingSample>>,
    /// Feedback buffer
    feedback_buffer: RwLock<VecDeque<PrefetchFeedback>>,
    /// Training configuration
    config: TrainingConfig,
    /// Training statistics
    stats: TrainingStats,
}

/// Training configuration
#[derive(Debug, Clone, Copy)]
pub struct TrainingConfig {
    /// Base learning rate
    pub learning_rate: f32,
    /// Minimum learning rate (for decay)
    pub min_learning_rate: f32,
    /// Learning rate decay factor
    pub learning_rate_decay: f32,
    /// Batch size for updates (samples per update)
    pub batch_size: usize,
    /// Maximum samples in buffer
    pub max_buffer_size: usize,
    /// Enable online training
    pub enabled: bool,
    /// Update frequency (updates per second)
    pub updates_per_second: u32,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.001,
            min_learning_rate: 0.0001,
            learning_rate_decay: 0.99,
            batch_size: 32,
            max_buffer_size: 1024,
            enabled: true,
            updates_per_second: 10,
        }
    }
}

impl OnlineTrainer {
    /// Create new online trainer
    pub fn new() -> Self {
        Self::with_config(TrainingConfig::default())
    }

    /// Create trainer with custom config
    pub fn with_config(config: TrainingConfig) -> Self {
        Self {
            sample_buffer: RwLock::new(VecDeque::with_capacity(config.max_buffer_size)),
            feedback_buffer: RwLock::new(VecDeque::with_capacity(1024)),
            config,
            stats: TrainingStats::new(),
        }
    }

    /// Add training sample to buffer
    ///
    /// This is called when we have ground truth for a prediction
    pub fn add_sample(&self, sample: TrainingSample) {
        if !self.config.enabled {
            return;
        }

        let mut buffer = self.sample_buffer.write();

        // Ring buffer: remove oldest if full
        if buffer.len() >= self.config.max_buffer_size {
            buffer.pop_front();
            self.stats.samples_dropped.fetch_add(1, Ordering::Relaxed);
        }

        buffer.push_back(sample);
        self.stats.samples_collected.fetch_add(1, Ordering::Relaxed);
    }

    /// Add prefetch feedback
    ///
    /// Called when we know if a prefetch prediction was correct
    pub fn add_feedback(&self, feedback: PrefetchFeedback) {
        if !self.config.enabled {
            return;
        }

        let mut buffer = self.feedback_buffer.write();

        if buffer.len() >= 1024 {
            buffer.pop_front();
        }

        buffer.push_back(feedback);

        if feedback.was_hit {
            self.stats.prefetch_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.prefetch_misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Perform training update on model
    ///
    /// This should be called periodically (e.g., every 100ms)
    ///
    /// # Performance
    /// Target: < 1ms for batch of 32 samples
    pub fn train_step(&self, model: &mut QuantizedModel) -> TrainingMetrics {
        let start = crate::time::uptime_ns();

        if !self.config.enabled {
            return TrainingMetrics::default();
        }

        let mut sample_buffer = self.sample_buffer.write();

        if sample_buffer.len() < self.config.batch_size {
            // Not enough samples yet
            return TrainingMetrics::default();
        }

        // Extract batch
        let mut batch = Vec::with_capacity(self.config.batch_size);
        for _ in 0..self.config.batch_size {
            if let Some(sample) = sample_buffer.pop_front() {
                batch.push(sample);
            }
        }

        drop(sample_buffer);

        if batch.is_empty() {
            return TrainingMetrics::default();
        }

        // Compute adaptive learning rate based on recent accuracy
        let learning_rate = self.adaptive_learning_rate();

        // Train on batch
        let mut total_loss = 0.0f32;
        for sample in &batch {
            // Forward pass
            let output = model.infer(&sample.features);

            // Compute loss (MSE)
            let mut loss = 0.0f32;
            for i in 0..8 {
                let error = output[i] - sample.target[i];
                loss += error * error;
            }
            total_loss += loss;

            // Backward pass (simplified gradient descent)
            model.update_weights(&sample.features, &sample.target, learning_rate * sample.weight);
        }

        let avg_loss = total_loss / batch.len() as f32;

        let elapsed = crate::time::uptime_ns() - start;

        // Update statistics
        self.stats.training_steps.fetch_add(1, Ordering::Relaxed);
        self.stats.samples_trained.fetch_add(batch.len() as u64, Ordering::Relaxed);
        self.stats.total_training_time_ns.fetch_add(elapsed, Ordering::Relaxed);

        TrainingMetrics {
            samples_trained: batch.len(),
            avg_loss,
            learning_rate,
            elapsed_ns: elapsed,
        }
    }

    /// Compute adaptive learning rate based on recent performance
    fn adaptive_learning_rate(&self) -> f32 {
        let hits = self.stats.prefetch_hits.load(Ordering::Relaxed);
        let misses = self.stats.prefetch_misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total < 100 {
            // Not enough data yet, use base rate
            return self.config.learning_rate;
        }

        let accuracy = hits as f32 / total as f32;

        // Adjust learning rate based on accuracy
        let mut lr = self.config.learning_rate;

        if accuracy < 0.5 {
            // Poor accuracy - increase learning rate (model needs more learning)
            lr *= 1.2;
        } else if accuracy > 0.8 {
            // Good accuracy - decrease learning rate (model is converging)
            lr *= 0.8;
        }

        // Apply decay
        let steps = self.stats.training_steps.load(Ordering::Relaxed);
        let decay_factor = powi_approx(self.config.learning_rate_decay, (steps / 100) as i32);
        lr *= decay_factor;

        // Clamp to min
        lr.max(self.config.min_learning_rate)
    }

    /// Create training sample from prediction and actual outcome
    ///
    /// This is a helper to convert real-world feedback into training data
    pub fn create_training_sample(
        &self,
        features: [f32; 16],
        prediction: &PrefetchPrediction,
        was_correct: bool,
    ) -> TrainingSample {
        let mut target = [0.0f32; 8];

        // If prediction was correct, reinforce it
        // If incorrect, penalize it
        if was_correct {
            // Reinforce: set target to high confidence
            target[0] = 0.9;
            target[4] = prediction.offset as f32 / (1024.0 * 1024.0); // Normalized
        } else {
            // Penalize: set target to low confidence
            target[0] = 0.1;
        }

        // Weight by original confidence
        let weight = if was_correct {
            1.0
        } else {
            prediction.confidence // Higher penalty for high-confidence mistakes
        };

        TrainingSample {
            features,
            target,
            weight,
            timestamp: crate::time::uptime_ns(),
        }
    }

    /// Get current learning rate
    pub fn current_learning_rate(&self) -> f32 {
        self.adaptive_learning_rate()
    }

    /// Get accuracy from recent feedback
    pub fn recent_accuracy(&self) -> f32 {
        let hits = self.stats.prefetch_hits.load(Ordering::Relaxed);
        let misses = self.stats.prefetch_misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total == 0 {
            return 0.0;
        }

        hits as f32 / total as f32
    }

    /// Get training statistics
    pub fn stats(&self) -> TrainingStats {
        self.stats.clone()
    }

    /// Get configuration
    pub fn config(&self) -> TrainingConfig {
        self.config
    }

    /// Update configuration
    pub fn set_config(&mut self, config: TrainingConfig) {
        self.config = config;
    }

    /// Clear all buffers
    pub fn clear(&self) {
        let mut samples = self.sample_buffer.write();
        samples.clear();

        let mut feedback = self.feedback_buffer.write();
        feedback.clear();
    }

    /// Get buffer fill ratio (0.0 - 1.0)
    pub fn buffer_fill_ratio(&self) -> f32 {
        let samples = self.sample_buffer.read();
        samples.len() as f32 / self.config.max_buffer_size as f32
    }
}

impl Default for OnlineTrainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Training statistics
#[derive(Debug)]
pub struct TrainingStats {
    /// Total samples collected
    pub samples_collected: AtomicU64,
    /// Samples dropped (buffer full)
    pub samples_dropped: AtomicU64,
    /// Samples actually used for training
    pub samples_trained: AtomicU64,
    /// Training steps performed
    pub training_steps: AtomicU64,
    /// Total training time (nanoseconds)
    pub total_training_time_ns: AtomicU64,
    /// Prefetch hits (correct predictions)
    pub prefetch_hits: AtomicU64,
    /// Prefetch misses (wrong predictions)
    pub prefetch_misses: AtomicU64,
}

impl TrainingStats {
    fn new() -> Self {
        Self {
            samples_collected: AtomicU64::new(0),
            samples_dropped: AtomicU64::new(0),
            samples_trained: AtomicU64::new(0),
            training_steps: AtomicU64::new(0),
            total_training_time_ns: AtomicU64::new(0),
            prefetch_hits: AtomicU64::new(0),
            prefetch_misses: AtomicU64::new(0),
        }
    }

    /// Average training time per step
    pub fn avg_training_time_ns(&self) -> u64 {
        let steps = self.training_steps.load(Ordering::Relaxed);
        if steps == 0 {
            return 0;
        }
        self.total_training_time_ns.load(Ordering::Relaxed) / steps
    }

    /// Sample utilization ratio
    pub fn sample_utilization(&self) -> f32 {
        let collected = self.samples_collected.load(Ordering::Relaxed);
        if collected == 0 {
            return 0.0;
        }
        let trained = self.samples_trained.load(Ordering::Relaxed);
        trained as f32 / collected as f32
    }

    /// Prefetch accuracy
    pub fn prefetch_accuracy(&self) -> f32 {
        let hits = self.prefetch_hits.load(Ordering::Relaxed);
        let misses = self.prefetch_misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total == 0 {
            return 0.0;
        }

        hits as f32 / total as f32
    }
}

impl Clone for TrainingStats {
    fn clone(&self) -> Self {
        Self {
            samples_collected: AtomicU64::new(self.samples_collected.load(Ordering::Relaxed)),
            samples_dropped: AtomicU64::new(self.samples_dropped.load(Ordering::Relaxed)),
            samples_trained: AtomicU64::new(self.samples_trained.load(Ordering::Relaxed)),
            training_steps: AtomicU64::new(self.training_steps.load(Ordering::Relaxed)),
            total_training_time_ns: AtomicU64::new(self.total_training_time_ns.load(Ordering::Relaxed)),
            prefetch_hits: AtomicU64::new(self.prefetch_hits.load(Ordering::Relaxed)),
            prefetch_misses: AtomicU64::new(self.prefetch_misses.load(Ordering::Relaxed)),
        }
    }
}

/// Metrics from a training step
#[derive(Debug, Clone, Copy)]
pub struct TrainingMetrics {
    /// Number of samples trained in this step
    pub samples_trained: usize,
    /// Average loss (MSE)
    pub avg_loss: f32,
    /// Learning rate used
    pub learning_rate: f32,
    /// Time elapsed (nanoseconds)
    pub elapsed_ns: u64,
}

impl Default for TrainingMetrics {
    fn default() -> Self {
        Self {
            samples_trained: 0,
            avg_loss: 0.0,
            learning_rate: 0.0,
            elapsed_ns: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trainer_creation() {
        let trainer = OnlineTrainer::new();
        assert!(trainer.config.enabled);
    }

    #[test]
    fn test_add_sample() {
        let trainer = OnlineTrainer::new();

        let sample = TrainingSample {
            features: [0.0; 16],
            target: [0.0; 8],
            weight: 1.0,
            timestamp: crate::time::uptime_ns(),
        };

        trainer.add_sample(sample);

        let stats = trainer.stats();
        assert_eq!(stats.samples_collected.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_feedback_tracking() {
        let trainer = OnlineTrainer::new();

        let feedback = PrefetchFeedback {
            predicted_offset: 0,
            was_hit: true,
            confidence: 0.8,
            time_to_access_ns: Some(1000),
        };

        trainer.add_feedback(feedback);

        let stats = trainer.stats();
        assert_eq!(stats.prefetch_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.prefetch_misses.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_accuracy_computation() {
        let trainer = OnlineTrainer::new();

        // Add some hits and misses
        for _ in 0..8 {
            trainer.add_feedback(PrefetchFeedback {
                predicted_offset: 0,
                was_hit: true,
                confidence: 0.8,
                time_to_access_ns: Some(1000),
            });
        }

        for _ in 0..2 {
            trainer.add_feedback(PrefetchFeedback {
                predicted_offset: 0,
                was_hit: false,
                confidence: 0.8,
                time_to_access_ns: None,
            });
        }

        let accuracy = trainer.recent_accuracy();
        assert!((accuracy - 0.8).abs() < 0.01); // 8/10 = 0.8
    }

    #[test]
    fn test_buffer_overflow() {
        let mut config = TrainingConfig::default();
        config.max_buffer_size = 10;
        let trainer = OnlineTrainer::with_config(config);

        // Add more samples than buffer size
        for i in 0..20 {
            let sample = TrainingSample {
                features: [i as f32; 16],
                target: [0.0; 8],
                weight: 1.0,
                timestamp: crate::time::uptime_ns(),
            };
            trainer.add_sample(sample);
        }

        let stats = trainer.stats();
        assert_eq!(stats.samples_collected.load(Ordering::Relaxed), 20);
        assert_eq!(stats.samples_dropped.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn test_adaptive_learning_rate() {
        let trainer = OnlineTrainer::new();

        // High accuracy should decrease learning rate
        for _ in 0..100 {
            trainer.add_feedback(PrefetchFeedback {
                predicted_offset: 0,
                was_hit: true,
                confidence: 0.9,
                time_to_access_ns: Some(1000),
            });
        }

        let lr_high_accuracy = trainer.current_learning_rate();

        // Clear and test low accuracy
        trainer.clear();

        for _ in 0..100 {
            trainer.add_feedback(PrefetchFeedback {
                predicted_offset: 0,
                was_hit: false,
                confidence: 0.5,
                time_to_access_ns: None,
            });
        }

        let lr_low_accuracy = trainer.current_learning_rate();

        // Low accuracy should have higher learning rate
        assert!(lr_low_accuracy >= lr_high_accuracy);
    }
}
