//! Validator - Integrity validation hooks
//!
//! ## Features
//! - Pre/post operation validation
//! - Custom validation rules
//! - Hook registration
//! - Performance tracking
//!
//! ## Use Cases
//! - Enforce data format constraints
//! - Detect anomalies
//! - Audit trail
//! - Policy enforcement

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::{FsError, FsResult};

/// Validation context
#[derive(Debug, Clone)]
pub struct ValidationContext {
    pub device_id: u64,
    pub inode: u64,
    pub operation: &'static str,
    pub user_data: u64,
}

impl ValidationContext {
    pub fn new(device_id: u64, inode: u64, operation: &'static str) -> Self {
        Self {
            device_id,
            inode,
            operation,
            user_data: 0,
        }
    }
}

/// Validation result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Allow,
    Deny(ValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub code: u32,
    pub message: alloc::string::String,
}

impl ValidationError {
    pub fn new(code: u32, message: &str) -> Self {
        Self {
            code,
            message: alloc::string::String::from(message),
        }
    }
}

/// Integrity validator trait
pub trait IntegrityValidator: Send + Sync {
    /// Pre-operation validation
    fn validate_pre(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult;

    /// Post-operation validation
    fn validate_post(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult;

    /// Validator name
    fn name(&self) -> &str;
}

/// Checksum validator
pub struct ChecksumValidator {
    /// Expected checksum
    expected_checksums: RwLock<alloc::collections::BTreeMap<u64, super::checksum::Blake3Hash>>,
}

impl ChecksumValidator {
    pub fn new() -> Self {
        Self {
            expected_checksums: RwLock::new(alloc::collections::BTreeMap::new()),
        }
    }

    pub fn set_expected(&self, inode: u64, hash: super::checksum::Blake3Hash) {
        self.expected_checksums.write().insert(inode, hash);
    }
}

impl Default for ChecksumValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl IntegrityValidator for ChecksumValidator {
    fn validate_pre(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        // Pre-validation: check if data matches expected checksum
        let checksums = self.expected_checksums.read();

        if let Some(expected) = checksums.get(&ctx.inode) {
            let actual = super::checksum::compute_blake3(data);

            if actual != *expected {
                return ValidationResult::Deny(ValidationError::new(
                    1,
                    "Checksum mismatch in pre-validation",
                ));
            }
        }

        ValidationResult::Allow
    }

    fn validate_post(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        // Post-validation: verify data integrity after operation
        let checksums = self.expected_checksums.read();

        if let Some(expected) = checksums.get(&ctx.inode) {
            let actual = super::checksum::compute_blake3(data);

            if actual != *expected {
                return ValidationResult::Deny(ValidationError::new(
                    2,
                    "Checksum mismatch in post-validation",
                ));
            }
        }

        ValidationResult::Allow
    }

    fn name(&self) -> &str {
        "ChecksumValidator"
    }
}

/// Size validator
pub struct SizeValidator {
    max_size: usize,
}

impl SizeValidator {
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }
}

impl IntegrityValidator for SizeValidator {
    fn validate_pre(&self, _ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        if data.len() > self.max_size {
            return ValidationResult::Deny(ValidationError::new(
                3,
                "Data exceeds maximum size",
            ));
        }

        ValidationResult::Allow
    }

    fn validate_post(&self, _ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        if data.len() > self.max_size {
            return ValidationResult::Deny(ValidationError::new(
                4,
                "Result exceeds maximum size",
            ));
        }

        ValidationResult::Allow
    }

    fn name(&self) -> &str {
        "SizeValidator"
    }
}

/// Validator registry
pub struct ValidatorRegistry {
    /// Registered validators
    validators: RwLock<Vec<Arc<dyn IntegrityValidator>>>,
    /// Statistics
    stats: ValidatorStats,
}

#[derive(Debug, Default)]
pub struct ValidatorStats {
    pub validations_performed: AtomicU64,
    pub validations_passed: AtomicU64,
    pub validations_failed: AtomicU64,
}

impl ValidatorRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            validators: RwLock::new(Vec::new()),
            stats: ValidatorStats::default(),
        })
    }

    /// Register validator
    pub fn register(&self, validator: Arc<dyn IntegrityValidator>) {
        log::info!("validator: registering validator '{}'", validator.name());
        self.validators.write().push(validator);
    }

    /// Unregister validator
    pub fn unregister(&self, name: &str) {
        let mut validators = self.validators.write();
        validators.retain(|v| v.name() != name);
        log::info!("validator: unregistered validator '{}'", name);
    }

    /// Run pre-validation
    pub fn validate_pre(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        self.stats.validations_performed.fetch_add(1, Ordering::Relaxed);

        let validators = self.validators.read();

        for validator in validators.iter() {
            match validator.validate_pre(ctx, data) {
                ValidationResult::Allow => continue,
                ValidationResult::Deny(err) => {
                    log::warn!(
                        "validator: pre-validation failed by '{}': {}",
                        validator.name(),
                        err.message
                    );
                    self.stats.validations_failed.fetch_add(1, Ordering::Relaxed);
                    return ValidationResult::Deny(err);
                }
            }
        }

        self.stats.validations_passed.fetch_add(1, Ordering::Relaxed);
        ValidationResult::Allow
    }

    /// Run post-validation
    pub fn validate_post(&self, ctx: &ValidationContext, data: &[u8]) -> ValidationResult {
        self.stats.validations_performed.fetch_add(1, Ordering::Relaxed);

        let validators = self.validators.read();

        for validator in validators.iter() {
            match validator.validate_post(ctx, data) {
                ValidationResult::Allow => continue,
                ValidationResult::Deny(err) => {
                    log::warn!(
                        "validator: post-validation failed by '{}': {}",
                        validator.name(),
                        err.message
                    );
                    self.stats.validations_failed.fetch_add(1, Ordering::Relaxed);
                    return ValidationResult::Deny(err);
                }
            }
        }

        self.stats.validations_passed.fetch_add(1, Ordering::Relaxed);
        ValidationResult::Allow
    }

    pub fn stats(&self) -> &ValidatorStats {
        &self.stats
    }
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self {
            validators: RwLock::new(Vec::new()),
            stats: ValidatorStats::default(),
        }
    }
}

/// Global validator registry
static GLOBAL_VALIDATORS: spin::Once<Arc<ValidatorRegistry>> = spin::Once::new();

pub fn init() {
    GLOBAL_VALIDATORS.call_once(|| {
        log::info!("Initializing validator registry");
        let registry = ValidatorRegistry::new();

        // Register default validators
        registry.register(Arc::new(ChecksumValidator::new()));
        registry.register(Arc::new(SizeValidator::new(100 * 1024 * 1024))); // 100 MB max

        registry
    });
}

pub fn global_validators() -> &'static Arc<ValidatorRegistry> {
    GLOBAL_VALIDATORS.get().expect("Validator registry not initialized")
}
