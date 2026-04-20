//! Model update protocol for the exo_shield ML pipeline.
//!
//! Implements weight update, version tracking, update verification, and
//! rollback capability — all with static arrays and atomic counters.

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

use super::features::FEATURE_COUNT;
use super::model::{ActivationFn, ModelWeights, OUTPUT_SIZE};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of stored rollback snapshots.
pub const MAX_ROLLBACK_SNAPSHOTS: usize = 4;

/// Maximum size of an update payload (flat weight array + bias).
pub const UPDATE_PAYLOAD_SIZE: usize = FEATURE_COUNT * OUTPUT_SIZE + OUTPUT_SIZE;

// ---------------------------------------------------------------------------
// Model version
// ---------------------------------------------------------------------------

/// Version descriptor for a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ModelVersion {
    /// Monotonically increasing version number.
    version: u32,
    /// Checksum of the weight data (simple XOR-based).
    checksum: u64,
}

impl ModelVersion {
    pub const fn new(version: u32, checksum: u64) -> Self {
        Self { version, checksum }
    }

    pub fn version(&self) -> u32 { self.version }
    pub fn checksum(&self) -> u64 { self.checksum }

    /// Compute a simple XOR checksum over the model weights and biases.
    pub fn compute_checksum(model: &ModelWeights) -> u64 {
        let mut hash: u64 = 0x5175_696C_6C00_0000; // magic seed
        let weights = model.weight_matrix().as_flat();
        for chunk in weights.chunks(2) {
            let lo = chunk[0] as u64;
            let hi = if chunk.len() > 1 { chunk[1] as u64 } else { 0 };
            hash ^= lo | (hi << 32);
            hash = hash.wrapping_mul(0x5175_696C_6C00_0001);
        }
        for &b in model.bias() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x5175_696C_6C00_0002);
        }
        hash ^= model.version() as u64;
        hash
    }
}

// ---------------------------------------------------------------------------
// Update status
// ---------------------------------------------------------------------------

/// Status of a model update attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum UpdateStatus {
    /// Update applied successfully.
    Applied = 0,
    /// Verification failed (checksum mismatch).
    VerificationFailed = 1,
    /// Version regression detected (new version ≤ current).
    VersionRegression = 2,
    /// Update payload is invalid (e.g. out-of-range weights).
    InvalidPayload = 3,
    /// No rollback snapshot available.
    NoRollbackAvailable = 4,
    /// Rollback successful.
    RollbackOk = 5,
}

impl UpdateStatus {
    pub fn is_ok(self) -> bool {
        self == UpdateStatus::Applied || self == UpdateStatus::RollbackOk
    }
}

// ---------------------------------------------------------------------------
// Model update payload
// ---------------------------------------------------------------------------

/// A model update payload: flat weights + biases + metadata.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ModelUpdate {
    /// Flat weight data (FEATURE_COUNT * OUTPUT_SIZE elements, Q16.16).
    weights: [i32; FEATURE_COUNT * OUTPUT_SIZE],
    /// Bias values (OUTPUT_SIZE elements, Q16.16).
    bias: [i32; OUTPUT_SIZE],
    /// Target version for this update.
    target_version: u32,
    /// Activation function to set.
    activation: ActivationFn,
    /// Checksum claimed by the update source.
    claimed_checksum: u64,
}

impl ModelUpdate {
    /// Create a new update from flat arrays.
    pub const fn new(
        weights: [i32; FEATURE_COUNT * OUTPUT_SIZE],
        bias: [i32; OUTPUT_SIZE],
        target_version: u32,
        activation: ActivationFn,
        claimed_checksum: u64,
    ) -> Self {
        Self {
            weights,
            bias,
            target_version,
            activation,
            claimed_checksum,
        }
    }

    /// Validate that all weights and biases are within the allowed Q16.16
    /// range.  Returns `true` if the payload looks sane.
    pub fn validate(&self) -> bool {
        // Check for obviously corrupt values (all zero is technically valid
        // but unusual; we check for i32::MIN/MAX which are likely errors).
        const FORBIDDEN: i32 = i32::MIN;
        for &w in &self.weights {
            if w == FORBIDDEN {
                return false;
            }
        }
        for &b in &self.bias {
            if b == FORBIDDEN {
                return false;
            }
        }
        // Target version must be > 0.
        if self.target_version == 0 {
            return false;
        }
        true
    }

    pub fn target_version(&self) -> u32 { self.target_version }
    pub fn claimed_checksum(&self) -> u64 { self.claimed_checksum }
    pub fn activation(&self) -> ActivationFn { self.activation }
}

// ---------------------------------------------------------------------------
// Rollback snapshot
// ---------------------------------------------------------------------------

/// A snapshot of model weights used for rollback.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct RollbackSnapshot {
    model: ModelWeights,
    version: ModelVersion,
    valid: bool,
}

impl RollbackSnapshot {
    const fn empty() -> Self {
        Self {
            model: ModelWeights::zero(),
            version: ModelVersion::new(0, 0),
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Model update manager
// ---------------------------------------------------------------------------

/// Manages model updates with verification and rollback.
pub struct ModelUpdateManager {
    /// Current model weights.
    current: ModelWeights,
    /// Current version descriptor.
    current_version: ModelVersion,
    /// Rollback snapshot ring buffer.
    snapshots: [RollbackSnapshot; MAX_ROLLBACK_SNAPSHOTS],
    /// Ring-buffer write index.
    snapshot_idx: u32,
    /// Number of valid snapshots.
    snapshot_count: u32,
    /// Whether an update is in progress (simple lock).
    updating: AtomicBool,
    /// Total updates applied.
    total_updates: AtomicU32,
    /// Total rollbacks performed.
    total_rollbacks: AtomicU32,
}

impl ModelUpdateManager {
    /// Create a new manager with the given initial model.
    pub fn new(initial: ModelWeights) -> Self {
        let version = ModelVersion::compute_checksum(&initial);
        let current_version = ModelVersion::new(initial.version(), version);
        Self {
            current: initial,
            current_version,
            snapshots: [RollbackSnapshot::empty(); MAX_ROLLBACK_SNAPSHOTS],
            snapshot_idx: 0,
            snapshot_count: 0,
            updating: AtomicBool::new(false),
            total_updates: AtomicU32::new(0),
            total_rollbacks: AtomicU32::new(0),
        }
    }

    /// Get a reference to the current model.
    pub fn current_model(&self) -> &ModelWeights {
        &self.current
    }

    /// Get the current version.
    pub fn current_version(&self) -> &ModelVersion {
        &self.current_version
    }

    /// Apply a model update with verification.
    pub fn apply_update(&mut self, update: &ModelUpdate) -> UpdateStatus {
        // Acquire simple lock.
        if self.updating.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            // Already updating; reject.
            return UpdateStatus::InvalidPayload;
        }

        let result = self.apply_update_inner(update);

        // Release lock.
        self.updating.store(false, Ordering::Release);
        result
    }

    fn apply_update_inner(&mut self, update: &ModelUpdate) -> UpdateStatus {
        // 1. Validate payload.
        if !update.validate() {
            return UpdateStatus::InvalidPayload;
        }

        // 2. Check version regression.
        if update.target_version() <= self.current_version.version() {
            return UpdateStatus::VersionRegression;
        }

        // 3. Take a rollback snapshot of the current model.
        self.take_snapshot();

        // 4. Apply the new weights.
        let mut new_model = ModelWeights::zero();
        // Copy weights into the model.
        let flat = new_model.weight_matrix_mut().as_flat_mut();
        let mut i = 0;
        while i < FEATURE_COUNT * OUTPUT_SIZE {
            flat[i] = update.weights[i];
            i += 1;
        }
        new_model.set_bias(update.bias);
        new_model.set_activation(update.activation());
        new_model.set_version(update.target_version());

        // 5. Verify checksum.
        let computed = ModelVersion::compute_checksum(&new_model);
        if computed != update.claimed_checksum() {
            // Rollback automatically.
            self.rollback_inner();
            return UpdateStatus::VerificationFailed;
        }

        // 6. Commit.
        self.current = new_model;
        self.current_version = ModelVersion::new(update.target_version(), computed);
        self.total_updates.fetch_add(1, Ordering::Relaxed);
        UpdateStatus::Applied
    }

    /// Rollback to the most recent snapshot.
    pub fn rollback(&mut self) -> UpdateStatus {
        if self.updating.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            return UpdateStatus::InvalidPayload;
        }

        let result = self.rollback_inner();
        self.updating.store(false, Ordering::Release);
        result
    }

    fn rollback_inner(&mut self) -> UpdateStatus {
        // Find the most recent valid snapshot.
        for k in 0..self.snapshot_count.min(MAX_ROLLBACK_SNAPSHOTS as u32) {
            // Walk backwards from the current index.
            let idx = if self.snapshot_idx >= k + 1 {
                (self.snapshot_idx - k - 1) as usize
            } else {
                (MAX_ROLLBACK_SNAPSHOTS + self.snapshot_idx as usize - k as usize - 1)
                    % MAX_ROLLBACK_SNAPSHOTS
            };
            let snap = &self.snapshots[idx % MAX_ROLLBACK_SNAPSHOTS];
            if snap.valid {
                self.current = snap.model;
                self.current_version = snap.version;
                self.total_rollbacks.fetch_add(1, Ordering::Relaxed);
                return UpdateStatus::RollbackOk;
            }
        }
        UpdateStatus::NoRollbackAvailable
    }

    /// Take a snapshot of the current model state.
    fn take_snapshot(&mut self) {
        let idx = (self.snapshot_idx as usize) % MAX_ROLLBACK_SNAPSHOTS;
        self.snapshots[idx] = RollbackSnapshot {
            model: self.current,
            version: self.current_version,
            valid: true,
        };
        self.snapshot_idx += 1;
        if self.snapshot_count < MAX_ROLLBACK_SNAPSHOTS as u32 {
            self.snapshot_count += 1;
        }
    }

    /// Total updates applied.
    pub fn total_updates(&self) -> u32 {
        self.total_updates.load(Ordering::Relaxed)
    }

    /// Total rollbacks performed.
    pub fn total_rollbacks(&self) -> u32 {
        self.total_rollbacks.load(Ordering::Relaxed)
    }

    /// Number of valid rollback snapshots.
    pub fn snapshot_count(&self) -> u32 {
        self.snapshot_count
    }

    /// Whether an update is currently in progress.
    pub fn is_updating(&self) -> bool {
        self.updating.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_checksum_deterministic() {
        let m = ModelWeights::new_seeded(42, ActivationFn::Relu);
        let c1 = ModelVersion::compute_checksum(&m);
        let c2 = ModelVersion::compute_checksum(&m);
        assert_eq!(c1, c2);
    }

    #[test]
    fn apply_valid_update() {
        let initial = ModelWeights::new_seeded(1, ActivationFn::Relu);
        let mut mgr = ModelUpdateManager::new(initial);
        assert_eq!(mgr.current_version().version(), 1);

        // Build a valid update.
        let mut weights = [0i32; FEATURE_COUNT * OUTPUT_SIZE];
        for i in 0..weights.len() {
            weights[i] = (i as i32) % 1000;
        }
        let bias = [0i32; OUTPUT_SIZE];
        let target_version = 2u32;

        // Compute correct checksum.
        let mut tmp_model = ModelWeights::zero();
        let flat = tmp_model.weight_matrix_mut().as_flat_mut();
        let mut i = 0;
        while i < FEATURE_COUNT * OUTPUT_SIZE {
            flat[i] = weights[i];
            i += 1;
        }
        tmp_model.set_bias(bias);
        tmp_model.set_activation(ActivationFn::Sigmoid);
        tmp_model.set_version(target_version);
        let checksum = ModelVersion::compute_checksum(&tmp_model);

        let update = ModelUpdate::new(weights, bias, target_version, ActivationFn::Sigmoid, checksum);
        let status = mgr.apply_update(&update);
        assert_eq!(status, UpdateStatus::Applied);
        assert_eq!(mgr.current_model().version(), 2);
        assert_eq!(mgr.total_updates(), 1);
        assert_eq!(mgr.snapshot_count(), 1);
    }

    #[test]
    fn reject_version_regression() {
        let initial = ModelWeights::new_seeded(1, ActivationFn::Relu);
        // The seeded model has version 1
        let mut mgr = ModelUpdateManager::new(initial);

        let weights = [0i32; FEATURE_COUNT * OUTPUT_SIZE];
        let bias = [0i32; OUTPUT_SIZE];
        let update = ModelUpdate::new(weights, bias, 1, ActivationFn::Relu, 0);
        let status = mgr.apply_update(&update);
        assert_eq!(status, UpdateStatus::VersionRegression);
    }

    #[test]
    fn reject_bad_checksum() {
        let initial = ModelWeights::new_seeded(1, ActivationFn::Relu);
        let mut mgr = ModelUpdateManager::new(initial);

        let weights = [100i32; FEATURE_COUNT * OUTPUT_SIZE];
        let bias = [50i32; OUTPUT_SIZE];
        let update = ModelUpdate::new(weights, bias, 5, ActivationFn::Relu, 0xDEAD_BEEF);
        let status = mgr.apply_update(&update);
        assert_eq!(status, UpdateStatus::VerificationFailed);
    }

    #[test]
    fn rollback() {
        let initial = ModelWeights::new_seeded(1, ActivationFn::Relu);
        let initial_checksum = ModelVersion::compute_checksum(&initial);
        let mut mgr = ModelUpdateManager::new(initial);

        // Apply a valid update
        let mut weights = [0i32; FEATURE_COUNT * OUTPUT_SIZE];
        for i in 0..weights.len() {
            weights[i] = (i as i32) % 500;
        }
        let bias = [0i32; OUTPUT_SIZE];
        let target_v = 2u32;

        let mut tmp = ModelWeights::zero();
        let flat = tmp.weight_matrix_mut().as_flat_mut();
        let mut i = 0;
        while i < FEATURE_COUNT * OUTPUT_SIZE {
            flat[i] = weights[i];
            i += 1;
        }
        tmp.set_bias(bias);
        tmp.set_activation(ActivationFn::Relu);
        tmp.set_version(target_v);
        let cs = ModelVersion::compute_checksum(&tmp);

        let update = ModelUpdate::new(weights, bias, target_v, ActivationFn::Relu, cs);
        assert_eq!(mgr.apply_update(&update), UpdateStatus::Applied);

        // Rollback
        let status = mgr.rollback();
        assert_eq!(status, UpdateStatus::RollbackOk);
        assert_eq!(mgr.current_model().version(), 1);
        let after_cs = ModelVersion::compute_checksum(mgr.current_model());
        assert_eq!(after_cs, initial_checksum);
        assert_eq!(mgr.total_rollbacks(), 1);
    }

    #[test]
    fn update_payload_validation() {
        let mut weights = [0i32; FEATURE_COUNT * OUTPUT_SIZE];
        let bias = [0i32; OUTPUT_SIZE];
        weights[0] = i32::MIN; // forbidden value
        let update = ModelUpdate::new(weights, bias, 2, ActivationFn::Relu, 0);
        assert!(!update.validate());
    }
}
