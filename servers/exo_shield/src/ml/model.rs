//! Simple neural-network model for the exo_shield ML pipeline.
//!
//! Implements a single-layer feedforward network: 32 inputs → 16 outputs
//! with configurable activation, stored as static arrays.

use core::sync::atomic::{AtomicU32, Ordering};

use super::features::{FeatureVector, FEATURE_COUNT};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of output neurons.
pub const OUTPUT_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Activation functions
// ---------------------------------------------------------------------------

/// Supported activation functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum ActivationFn {
    /// ReLU: max(0, x)
    Relu = 0,
    /// Sigmoid approximation in Q16.16 fixed-point.
    Sigmoid = 1,
    /// Leaky ReLU: max(α·x, x)
    LeakyRelu = 2,
    /// Tanh approximation in Q16.16.
    Tanh = 3,
    /// Linear (identity).
    Linear = 4,
}

impl ActivationFn {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ActivationFn::Relu),
            1 => Some(ActivationFn::Sigmoid),
            2 => Some(ActivationFn::LeakyRelu),
            3 => Some(ActivationFn::Tanh),
            4 => Some(ActivationFn::Linear),
            _ => None,
        }
    }

    /// Apply the activation function to a Q16.16 value.
    pub fn apply(&self, x: i32) -> i32 {
        match self {
            ActivationFn::Relu => x.max(0),
            ActivationFn::Sigmoid => sigmoid_approx(x),
            ActivationFn::LeakyRelu => {
                if x >= 0 {
                    x
                } else {
                    // α = 0.01 in Q16.16 ≈ 655
                    ((x as i64) * 655 >> 16) as i32
                }
            }
            ActivationFn::Tanh => tanh_approx(x),
            ActivationFn::Linear => x,
        }
    }
}

/// Sigmoid approximation in Q16.16 using a piecewise-linear model.
/// σ(x) ≈ 0.5 + x/4  for |x| < 2 (in Q16.16)
/// σ(x) → 1  for x >> 0
/// σ(x) → 0  for x << 0
fn sigmoid_approx(x: i32) -> i32 {
    const FP_ONE: i32 = 1 << 16;
    const HALF: i32 = 1 << 15; // 0.5 in Q16.16

    if x >= (4 << 16) {
        // x >= 4.0 → output ≈ 1.0
        return FP_ONE;
    }
    if x <= -(4 << 16) {
        // x <= -4.0 → output ≈ 0.0
        return 0;
    }
    // Piecewise-linear: σ(x) ≈ 0.5 + x/8  (steeper than x/4 for range)
    // In Q16.16: HALF + x/8  where x is Q16.16
    let term = (x as i64) / 8;
    let result = HALF + term as i32;
    result.clamp(0, FP_ONE)
}

/// Tanh approximation using sigmoid: tanh(x) = 2σ(2x) - 1.
fn tanh_approx(x: i32) -> i32 {
    const FP_ONE: i32 = 1 << 16;
    let sig = sigmoid_approx(2 * x);
    // 2*sig - 1 in Q16.16
    let result = 2 * sig as i64 - FP_ONE as i64;
    result.clamp(-FP_ONE as i64, FP_ONE as i64) as i32
}

// ---------------------------------------------------------------------------
// Weight matrix
// ---------------------------------------------------------------------------

/// 32×16 weight matrix stored in row-major order as Q16.16 fixed-point.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct WeightMatrix {
    /// Row-major: weights[row * OUTPUT_SIZE + col].
    weights: [i32; FEATURE_COUNT * OUTPUT_SIZE],
}

impl WeightMatrix {
    /// Create a zero-valued weight matrix.
    pub const fn zero() -> Self {
        Self {
            weights: [0i32; FEATURE_COUNT * OUTPUT_SIZE],
        }
    }

    /// Create with small random-ish init (simple LCG-based fill).
    /// The `seed` parameter drives the LCG for reproducibility.
    pub fn seeded_init(seed: u32) -> Self {
        let mut mat = Self::zero();
        let mut state = seed;
        for i in 0..FEATURE_COUNT * OUTPUT_SIZE {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            // Map to small Q16.16 values in [-0.5, 0.5) ≈ [-32768, 32768)
            let v = ((state >> 16) as i32) % 32768;
            mat.weights[i] = v - 16384; // centre around 0
        }
        mat
    }

    /// Get the weight at (row, col).
    pub fn get(&self, row: usize, col: usize) -> i32 {
        if row < FEATURE_COUNT && col < OUTPUT_SIZE {
            self.weights[row * OUTPUT_SIZE + col]
        } else {
            0
        }
    }

    /// Set the weight at (row, col).
    pub fn set(&mut self, row: usize, col: usize, val: i32) {
        if row < FEATURE_COUNT && col < OUTPUT_SIZE {
            self.weights[row * OUTPUT_SIZE + col] = val;
        }
    }

    /// Get a reference to the flat weight array.
    pub fn as_flat(&self) -> &[i32; FEATURE_COUNT * OUTPUT_SIZE] {
        &self.weights
    }

    /// Get a mutable reference to the flat weight array.
    pub fn as_flat_mut(&mut self) -> &mut [i32; FEATURE_COUNT * OUTPUT_SIZE] {
        &mut self.weights
    }
}

// ---------------------------------------------------------------------------
// Model weights (complete model state)
// ---------------------------------------------------------------------------

/// Complete set of model weights (single-layer perceptron: 32→16).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ModelWeights {
    /// Weight matrix.
    weights: WeightMatrix,
    /// Bias vector (one per output neuron) in Q16.16.
    bias: [i32; OUTPUT_SIZE],
    /// Activation function for the output layer.
    activation: ActivationFn,
    /// Model version number.
    version: u32,
}

impl ModelWeights {
    /// Create a zero-initialised model.
    pub const fn zero() -> Self {
        Self {
            weights: WeightMatrix::zero(),
            bias: [0i32; OUTPUT_SIZE],
            activation: ActivationFn::Relu,
            version: 0,
        }
    }

    /// Create a model with seeded initialisation.
    pub fn new_seeded(seed: u32, activation: ActivationFn) -> Self {
        Self {
            weights: WeightMatrix::seeded_init(seed),
            bias: [0i32; OUTPUT_SIZE],
            activation,
            version: 1,
        }
    }

    /// Perform a forward pass: output[j] = activation( Σ_i input[i]·W[i][j] + bias[j] )
    pub fn forward(&self, input: &FeatureVector) -> [i32; OUTPUT_SIZE] {
        let mut output = [0i32; OUTPUT_SIZE];

        for j in 0..OUTPUT_SIZE {
            let mut sum: i64 = self.bias[j] as i64;
            for i in 0..FEATURE_COUNT {
                let iv = input.get(i) as i64;
                let w = self.weights.get(i, j) as i64;
                // Q16.16 × Q16.16 = Q32.32; shift back by 16.
                sum += (iv * w) >> 16;
            }
            // Clamp to i32 before applying activation.
            let clamped = sum.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            output[j] = self.activation.apply(clamped);
        }
        output
    }

    /// Get the bias vector.
    pub fn bias(&self) -> &[i32; OUTPUT_SIZE] {
        &self.bias
    }

    /// Set the bias vector.
    pub fn set_bias(&mut self, bias: [i32; OUTPUT_SIZE]) {
        self.bias = bias;
    }

    /// Get the weight matrix.
    pub fn weight_matrix(&self) -> &WeightMatrix {
        &self.weights
    }

    /// Get the weight matrix (mutable).
    pub fn weight_matrix_mut(&mut self) -> &mut WeightMatrix {
        &mut self.weights
    }

    /// Get the activation function.
    pub fn activation(&self) -> ActivationFn {
        self.activation
    }

    /// Set the activation function.
    pub fn set_activation(&mut self, activation: ActivationFn) {
        self.activation = activation;
    }

    /// Get the model version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Set the model version.
    pub fn set_version(&mut self, version: u32) {
        self.version = version;
    }
}

// ---------------------------------------------------------------------------
// Inference result
// ---------------------------------------------------------------------------

/// Output of a single model inference pass.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct InferenceResult {
    /// Raw output activations (Q16.16).
    raw_output: [i32; OUTPUT_SIZE],
    /// Index of the highest-activation output neuron.
    max_index: u8,
    /// Value of the highest activation (Q16.16).
    max_value: i32,
    /// Anomaly probability (0–65536 in Q16.16, 0 = benign, 65536 = certain anomaly).
    anomaly_prob: i32,
    /// Model version used for this inference.
    model_version: u32,
}

impl InferenceResult {
    /// Build an inference result from raw model output.
    pub fn from_output(raw_output: &[i32; OUTPUT_SIZE], model_version: u32) -> Self {
        let mut max_idx: u8 = 0;
        let mut max_val: i32 = i32::MIN;
        for j in 0..OUTPUT_SIZE {
            if raw_output[j] > max_val {
                max_val = raw_output[j];
                max_idx = j as u8;
            }
        }
        // Anomaly probability: use the max activation clamped to [0, 65536].
        // If the model is trained such that higher activation → more anomalous,
        // this gives a direct probability-like value.
        let fp_one = 1i32 << 16;
        let anomaly_prob = max_val.clamp(0, fp_one);

        Self {
            raw_output: *raw_output,
            max_index: max_idx,
            max_value: max_val,
            anomaly_prob,
            model_version,
        }
    }

    pub fn raw_output(&self) -> &[i32; OUTPUT_SIZE] { &self.raw_output }
    pub fn max_index(&self) -> u8 { self.max_index }
    pub fn max_value(&self) -> i32 { self.max_value }
    pub fn anomaly_prob(&self) -> i32 { self.anomaly_prob }
    pub fn model_version(&self) -> u32 { self.model_version }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relu_activation() {
        assert_eq!(ActivationFn::Relu.apply(5 << 16), 5 << 16);
        assert_eq!(ActivationFn::Relu.apply(-3 << 16), 0);
    }

    #[test]
    fn sigmoid_range() {
        let fp_one = 1 << 16;
        for x in &[-4i32 << 16, -2 << 16, 0, 2 << 16, 4 << 16] {
            let s = sigmoid_approx(*x);
            assert!(s >= 0 && s <= fp_one, "sigmoid({}) = {}", *x, s);
        }
    }

    #[test]
    fn tanh_range() {
        let fp_one = 1 << 16;
        for x in &[-4i32 << 16, -2 << 16, 0, 2 << 16, 4 << 16] {
            let t = tanh_approx(*x);
            assert!(t >= -fp_one && t <= fp_one, "tanh({}) = {}", *x, t);
        }
    }

    #[test]
    fn forward_pass_shape() {
        let model = ModelWeights::new_seeded(42, ActivationFn::Relu);
        let fv = FeatureVector::from_raw([100i32; FEATURE_COUNT]);
        let output = model.forward(&fv);
        // Output should have OUTPUT_SIZE elements; they may be zero (ReLU
        // kills negatives) but the computation runs.
        assert_eq!(output.len(), OUTPUT_SIZE);
    }

    #[test]
    fn inference_result_max() {
        let output = [10i32, 50, 30, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let result = InferenceResult::from_output(&output, 1);
        assert_eq!(result.max_index(), 1);
        assert_eq!(result.max_value(), 50);
    }

    #[test]
    fn weight_matrix_seeded() {
        let m = WeightMatrix::seeded_init(123);
        // Not all zeros
        let mut any_nonzero = false;
        for i in 0..FEATURE_COUNT * OUTPUT_SIZE {
            if m.as_flat()[i] != 0 {
                any_nonzero = true;
                break;
            }
        }
        assert!(any_nonzero);
    }
}
