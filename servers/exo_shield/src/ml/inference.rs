//! Inference engine for the exo_shield ML pipeline.
//!
//! Provides batch inference, confidence scoring, threshold-based
//! classification, and anomaly probability calculation.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::features::{FeatureExtractor, FeatureVector, ProcessBehaviourData, FEATURE_COUNT};
use super::model::{InferenceResult, ModelWeights, OUTPUT_SIZE};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum batch size for a single batch-inference call.
pub const MAX_BATCH_SIZE: usize = 16;

/// Default anomaly threshold in Q16.16 (0.5 = 32768).
pub const DEFAULT_ANOMALY_THRESHOLD: i32 = 32768;

/// Default confidence threshold in Q16.16 (0.7 ≈ 45875).
pub const DEFAULT_CONFIDENCE_THRESHOLD: i32 = 45875;

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classification label for a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Classification {
    /// Process appears benign.
    Benign = 0,
    /// Process is suspicious — monitor more closely.
    Suspicious = 1,
    /// Process is likely malicious — take action.
    Malicious = 2,
    /// Insufficient data for classification.
    Unknown = 3,
}

impl Classification {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Classification::Benign),
            1 => Some(Classification::Suspicious),
            2 => Some(Classification::Malicious),
            3 => Some(Classification::Unknown),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Confidence score
// ---------------------------------------------------------------------------

/// Confidence score for a classification (Q16.16, 0–65536).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ConfidenceScore(i32);

impl ConfidenceScore {
    pub const fn new(raw: i32) -> Self {
        Self(raw.clamp(0, 1 << 16))
    }

    /// Confidence as a fraction of 65536.
    pub fn raw(self) -> i32 {
        self.0
    }

    /// Whether the confidence exceeds a threshold.
    pub fn exceeds(self, threshold: i32) -> bool {
        self.0 >= threshold
    }

    /// Confidence as a percentage (0–100), computed with integer math.
    pub fn percent(self) -> u8 {
        ((self.0 as u64 * 100 / 65536) as u8).min(100)
    }
}

// ---------------------------------------------------------------------------
// Batch inference result
// ---------------------------------------------------------------------------

/// Result for a single item in a batch inference pass.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct BatchInferenceItem {
    /// Feature vector used (snapshot).
    features: FeatureVector,
    /// Model inference result.
    result: InferenceResult,
    /// Classification label.
    classification: Classification,
    /// Confidence score.
    confidence: ConfidenceScore,
    /// Anomaly probability (Q16.16).
    anomaly_prob: i32,
}

impl BatchInferenceItem {
    pub fn features(&self) -> &FeatureVector { &self.features }
    pub fn result(&self) -> &InferenceResult { &self.result }
    pub fn classification(&self) -> Classification { self.classification }
    pub fn confidence(&self) -> ConfidenceScore { self.confidence }
    pub fn anomaly_prob(&self) -> i32 { self.anomaly_prob }
}

// ---------------------------------------------------------------------------
// Inference engine
// ---------------------------------------------------------------------------

/// The main inference engine: wraps a model and provides batch inference,
/// confidence scoring, and classification.
pub struct InferenceEngine {
    /// Current model weights.
    model: ModelWeights,
    /// Anomaly probability threshold (Q16.16).
    anomaly_threshold: i32,
    /// Confidence threshold (Q16.16).
    confidence_threshold: i32,
    /// Min-max normalisation parameters.
    feat_min: [i32; FEATURE_COUNT],
    feat_max: [i32; FEATURE_COUNT],
    /// Total inferences performed.
    total_inferences: AtomicU64,
    /// Total anomalies detected.
    total_anomalies: AtomicU64,
}

impl InferenceEngine {
    /// Create a new engine with the given model.
    pub fn new(model: ModelWeights) -> Self {
        Self {
            model,
            anomaly_threshold: DEFAULT_ANOMALY_THRESHOLD,
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            feat_min: [0i32; FEATURE_COUNT],
            feat_max: [100i32; FEATURE_COUNT],
            total_inferences: AtomicU64::new(0),
            total_anomalies: AtomicU64::new(0),
        }
    }

    /// Set normalisation parameters.
    pub fn set_normalisation(&mut self, min: [i32; FEATURE_COUNT], max: [i32; FEATURE_COUNT]) {
        self.feat_min = min;
        self.feat_max = max;
    }

    /// Set the anomaly threshold.
    pub fn set_anomaly_threshold(&mut self, threshold: i32) {
        self.anomaly_threshold = threshold.clamp(0, 1 << 16);
    }

    /// Set the confidence threshold.
    pub fn set_confidence_threshold(&mut self, threshold: i32) {
        self.confidence_threshold = threshold.clamp(0, 1 << 16);
    }

    /// Get the current anomaly threshold.
    pub fn anomaly_threshold(&self) -> i32 {
        self.anomaly_threshold
    }

    /// Get the current confidence threshold.
    pub fn confidence_threshold(&self) -> i32 {
        self.confidence_threshold
    }

    // -----------------------------------------------------------------------
    // Single inference
    // -----------------------------------------------------------------------

    /// Run inference on a single feature vector.
    pub fn infer(&self, features: &FeatureVector) -> BatchInferenceItem {
        let mut fv = *features;
        if !fv.is_normalised() {
            fv.normalise_minmax(&self.feat_min, &self.feat_max);
        }

        let result = InferenceResult::from_output(
            &self.model.forward(&fv),
            self.model.version(),
        );

        let anomaly_prob = result.anomaly_prob();
        let confidence = Self::compute_confidence(&result);
        let classification = Self::classify(anomaly_prob, confidence, self.anomaly_threshold, self.confidence_threshold);

        self.total_inferences.fetch_add(1, Ordering::Relaxed);
        if classification == Classification::Malicious || classification == Classification::Suspicious {
            self.total_anomalies.fetch_add(1, Ordering::Relaxed);
        }

        BatchInferenceItem {
            features: fv,
            result,
            classification,
            confidence,
            anomaly_prob,
        }
    }

    /// Run inference from raw process behaviour data.
    pub fn infer_from_behaviour(&self, data: &ProcessBehaviourData) -> BatchInferenceItem {
        let fv = FeatureExtractor::extract_normalised(data, &self.feat_min, &self.feat_max);
        self.infer(&fv)
    }

    // -----------------------------------------------------------------------
    // Batch inference
    // -----------------------------------------------------------------------

    /// Run batch inference on up to `MAX_BATCH_SIZE` feature vectors.
    /// Returns the number of items actually processed.
    pub fn infer_batch(
        &self,
        inputs: &[FeatureVector],
        results: &mut [BatchInferenceItem; MAX_BATCH_SIZE],
    ) -> usize {
        let n = inputs.len().min(MAX_BATCH_SIZE);
        for i in 0..n {
            results[i] = self.infer(&inputs[i]);
        }
        n
    }

    /// Run batch inference from raw behaviour data.
    pub fn infer_batch_behaviour(
        &self,
        inputs: &[ProcessBehaviourData],
        results: &mut [BatchInferenceItem; MAX_BATCH_SIZE],
    ) -> usize {
        let n = inputs.len().min(MAX_BATCH_SIZE);
        for i in 0..n {
            results[i] = self.infer_from_behaviour(&inputs[i]);
        }
        n
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    /// Total number of inferences performed.
    pub fn total_inferences(&self) -> u64 {
        self.total_inferences.load(Ordering::Relaxed)
    }

    /// Total number of anomalies detected.
    pub fn total_anomalies(&self) -> u64 {
        self.total_anomalies.load(Ordering::Relaxed)
    }

    /// Anomaly rate (anomalies / total) as a Q16.16 fraction.
    /// Returns 0 if no inferences have been performed.
    pub fn anomaly_rate(&self) -> i32 {
        let total = self.total_inferences.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let anomalies = self.total_anomalies.load(Ordering::Relaxed);
        // (anomalies << 16) / total  — use u64 for headroom
        ((anomalies as u64) << 16 / total as u64).clamp(0, u64::MAX) as i32
    }

    // -----------------------------------------------------------------------
    // Model access
    // -----------------------------------------------------------------------

    /// Get the current model.
    pub fn model(&self) -> &ModelWeights {
        &self.model
    }

    /// Replace the model (e.g. after an update).
    pub fn set_model(&mut self, model: ModelWeights) {
        self.model = model;
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Compute a confidence score from the inference result.
    ///
    /// Confidence is based on the margin between the top-1 and top-2
    /// activations.  A larger margin → higher confidence.
    fn compute_confidence(result: &InferenceResult) -> ConfidenceScore {
        let raw = result.raw_output();
        let mut top1: i64 = i64::MIN;
        let mut top2: i64 = i64::MIN;
        for &v in raw.iter() {
            let v = v as i64;
            if v > top1 {
                top2 = top1;
                top1 = v;
            } else if v > top2 {
                top2 = v;
            }
        }
        // Margin in Q16.16.
        let margin = top1 - top2;
        // Map margin to [0, 65536].  A margin of ~65536 (≈1.0) is fully
        // confident; a margin of 0 is not confident at all.
        let conf = if margin < 0 {
            0i32
        } else if margin >= (1i64 << 16) {
            (1i32 << 16)
        } else {
            margin as i32
        };
        ConfidenceScore::new(conf)
    }

    /// Classify based on anomaly probability and confidence.
    fn classify(
        anomaly_prob: i32,
        confidence: ConfidenceScore,
        anomaly_threshold: i32,
        confidence_threshold: i32,
    ) -> Classification {
        if anomaly_prob >= anomaly_threshold && confidence.exceeds(confidence_threshold) {
            Classification::Malicious
        } else if anomaly_prob >= anomaly_threshold {
            // High anomaly but low confidence → suspicious
            Classification::Suspicious
        } else if anomaly_prob >= anomaly_threshold / 2 {
            // Below threshold but above half → suspicious
            Classification::Suspicious
        } else {
            Classification::Benign
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::ActivationFn;

    #[test]
    fn single_inference() {
        let model = ModelWeights::new_seeded(42, ActivationFn::Relu);
        let engine = InferenceEngine::new(model);
        let fv = FeatureVector::from_raw([100i32; FEATURE_COUNT]);
        let item = engine.infer(&fv);
        // Classification should be one of the valid variants
        match item.classification() {
            Classification::Benign
            | Classification::Suspicious
            | Classification::Malicious
            | Classification::Unknown => {}
        }
    }

    #[test]
    fn batch_inference() {
        let model = ModelWeights::new_seeded(99, ActivationFn::Relu);
        let engine = InferenceEngine::new(model);
        let mut inputs = [FeatureVector::zero(); MAX_BATCH_SIZE];
        for i in 0..MAX_BATCH_SIZE {
            let mut fv = FeatureVector::zero();
            fv.set(0, (i as i32) * 100);
            inputs[i] = fv;
        }
        let mut results: [BatchInferenceItem; MAX_BATCH_SIZE] = unsafe {
            // Safe: we initialise all entries via infer_batch
            core::mem::zeroed()
        };
        let n = engine.infer_batch(&inputs, &mut results);
        assert_eq!(n, MAX_BATCH_SIZE);
        assert_eq!(engine.total_inferences(), MAX_BATCH_SIZE as u64);
    }

    #[test]
    fn confidence_score_percent() {
        let cs = ConfidenceScore::new(32768); // 0.5 in Q16.16
        let pct = cs.percent();
        assert!(pct >= 49 && pct <= 51, "percent = {}", pct);
    }

    #[test]
    fn classification_threshold() {
        // High anomaly + high confidence → Malicious
        let c = InferenceEngine::classify(
            50000, // > threshold
            ConfidenceScore::new(50000), // > confidence threshold
            DEFAULT_ANOMALY_THRESHOLD,
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(c, Classification::Malicious);

        // High anomaly + low confidence → Suspicious
        let c2 = InferenceEngine::classify(
            50000,
            ConfidenceScore::new(10000),
            DEFAULT_ANOMALY_THRESHOLD,
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(c2, Classification::Suspicious);

        // Low anomaly → Benign
        let c3 = InferenceEngine::classify(
            10000,
            ConfidenceScore::new(60000),
            DEFAULT_ANOMALY_THRESHOLD,
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(c3, Classification::Benign);
    }

    #[test]
    fn infer_from_behaviour() {
        let model = ModelWeights::new_seeded(7, ActivationFn::Sigmoid);
        let mut engine = InferenceEngine::new(model);
        // Set wide normalisation range
        engine.set_normalisation([0i32; FEATURE_COUNT], [10000i32; FEATURE_COUNT]);

        let mut data = ProcessBehaviourData::zero();
        data.syscall_rate = 500;
        data.ptrace_use = 3;
        let item = engine.infer_from_behaviour(&data);
        // Should produce a valid result
        assert!(item.anomaly_prob() >= 0);
    }
}
