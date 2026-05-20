//! Machine-learning module for the exo_shield security server.
//!
//! Provides feature extraction, a simple neural-network model (32→16),
//! an inference engine, and a model-update protocol with versioning and
//! rollback — all `no_std` compatible with static arrays only.

pub mod features;
pub mod inference;
pub mod model;
pub mod update;

// Re-export primary public types.
pub use features::{FeatureExtractor, FeatureVector};
pub use inference::{Classification, ConfidenceScore, InferenceEngine};
pub use model::{ActivationFn, InferenceResult, ModelWeights, WeightMatrix};
pub use update::{ModelUpdate, ModelUpdateManager, ModelVersion, UpdateStatus};
