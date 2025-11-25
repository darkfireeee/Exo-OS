//! Prediction heuristics for thread scheduling

use super::ema::EmaPredictor;

/// Prediction heuristics
pub struct PredictionHeuristics {
    ema: EmaPredictor,
}

impl PredictionHeuristics {
    pub fn new() -> Self {
        Self {
            ema: EmaPredictor::new(),
        }
    }
    
    /// Predict next runtime based on history
    pub fn predict_runtime(&self) -> u64 {
        self.ema.predict()
    }
    
    /// Update with actual runtime
    pub fn update_runtime(&self, actual_ns: u64) {
        self.ema.update(actual_ns);
    }
    
    /// Check if thread is likely I/O bound
    pub fn is_io_bound(&self) -> bool {
        let predicted = self.predict_runtime();
        predicted < 1_000_000 // < 1ms
    }
    
    /// Check if thread is likely CPU bound
    pub fn is_cpu_bound(&self) -> bool {
        let predicted = self.predict_runtime();
        predicted > 10_000_000 // > 10ms
    }
}

impl Default for PredictionHeuristics {
    fn default() -> Self {
        Self::new()
    }
}
