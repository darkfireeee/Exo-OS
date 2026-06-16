//! Module machine-learning d'ExoShield — architecture NGAV hybride.
//!
//! Composants :
//!   - `mlp`      : Deep MLP 32→128→64→4 (classifieur supervisé, Q16.16)
//!   - `iforest`  : Isolation Forest 8 arbres (détection zero-day non-supervisée)
//!   - `markov`   : Chaîne de Markov ordre-2 sur EventType (séquences anormales)
//!   - `ensemble` : Fusion pondérée MLP(0.45)+IF(0.35)+Markov(0.20) → score+classe
//!
//! Le perceptron original 32→16 et l'InferenceEngine sont conservés pour le
//! protocole de mise à jour de modèle (ModelUpdateManager/rollback).

pub mod ensemble;
pub mod features;
pub mod iforest;
pub mod inference;
pub mod markov;
pub mod mlp;
pub mod model;
pub mod trained_weights;
pub mod update;

// ── Re-exports originaux (compatibilité) ──────────────────────────────────────

pub use features::{FeatureExtractor, FeatureVector};
pub use inference::{Classification, ConfidenceScore, InferenceEngine};
pub use model::{ActivationFn, InferenceResult, ModelWeights, WeightMatrix};
pub use update::{ModelUpdate, ModelUpdateManager, ModelVersion, UpdateStatus};

// ── Re-exports ensemble (nouveau) ─────────────────────────────────────────────

pub use ensemble::{ensemble_classify, ensemble_init, ensemble_is_ready, EnsembleResult};
pub use iforest::{
    iforest_calibration_count, iforest_init, iforest_observe_normal, iforest_score,
};
pub use markov::{markov_clear_pid, markov_init, markov_observe, markov_total_events};
pub use mlp::{mlp_infer, mlp_init, mlp_update_weights, mlp_version, MlpWeights};
