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
pub use features::{FeatureVector, FeatureExtractor};
pub use model::{WeightMatrix, ModelWeights, InferenceResult, ActivationFn};
pub use inference::{InferenceEngine, Classification, ConfidenceScore};
pub use update::{ModelUpdate, ModelVersion, UpdateStatus, ModelUpdateManager};
