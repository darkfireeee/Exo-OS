//! Prediction algorithms for scheduler

pub mod ema;
pub mod heuristics;
pub mod history;

pub use ema::{EmaPredictor, EMA_ALPHA};
pub use heuristics::PredictionHeuristics;
pub use history::ExecutionHistory;
