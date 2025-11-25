//! EMA (Exponential Moving Average) prediction

use core::sync::atomic::{AtomicU64, Ordering};

/// EMA alpha parameter (0.25 = 25% weight to new value)
pub const EMA_ALPHA: f64 = 0.25;

/// EMA predictor for runtime prediction
#[derive(Debug)]
pub struct EmaPredictor {
    /// Current EMA value (in nanoseconds)
    ema_ns: AtomicU64,
    /// Number of samples
    samples: AtomicU64,
}

impl EmaPredictor {
    pub const fn new() -> Self {
        Self {
            ema_ns: AtomicU64::new(0),
            samples: AtomicU64::new(0),
        }
    }
    
    /// Update EMA with new runtime sample
    pub fn update(&self, runtime_ns: u64) {
        let samples = self.samples.fetch_add(1, Ordering::Relaxed);
        
        if samples == 0 {
            // First sample, just store it
            self.ema_ns.store(runtime_ns, Ordering::Relaxed);
        } else {
            // EMA update: new_ema = alpha * new + (1 - alpha) * old_ema
            let old_ema = self.ema_ns.load(Ordering::Relaxed);
            
            // Fixed-point arithmetic (multiply by 256 for precision)
            let alpha_fixed = (EMA_ALPHA * 256.0) as u64;
            let new_ema = (alpha_fixed * runtime_ns + (256 - alpha_fixed) * old_ema) / 256;
            
            self.ema_ns.store(new_ema, Ordering::Relaxed);
        }
    }
    
    /// Get current EMA prediction
    pub fn predict(&self) -> u64 {
        self.ema_ns.load(Ordering::Relaxed)
    }
    
    /// Get number of samples
    pub fn sample_count(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }
    
    /// Reset predictor
    pub fn reset(&self) {
        self.ema_ns.store(0, Ordering::Relaxed);
        self.samples.store(0, Ordering::Relaxed);
    }
}

impl Default for EmaPredictor {
    fn default() -> Self {
        Self::new()
    }
}
