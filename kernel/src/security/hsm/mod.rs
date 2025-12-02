//! Hardware Security Module (HSM) Support
//!
//! PKCS#11-style interface for external HSM devices.
//! Provides secure key storage, cryptographic operations, and attestation.

pub mod attestation;
pub mod crypto;
pub mod keys;
pub mod operations;
pub mod provider;
pub mod storage;

pub use attestation::{generate_attestation, Attestation, AttestationReport};
pub use keys::{generate_key, import_key, HsmKey, KeyHandle, KeyType, KeyUsage};
pub use operations::{decrypt_data, encrypt_data, sign_data, verify_signature, HsmOperation};
pub use provider::{detect_hsm, HsmProvider};
pub use storage::{read_slot, write_slot, SecureStorage, StorageSlot};

use spin::Mutex;

/// HSM state
static HSM_PROVIDER: Mutex<Option<HsmProvider>> = Mutex::new(None);

/// Initialize HSM subsystem
pub fn init() -> Result<(), &'static str> {
    log::info!("Initializing HSM subsystem...");

    if let Some(provider) = detect_hsm() {
        log::info!("HSM detected: {}", provider.name());
        *HSM_PROVIDER.lock() = Some(provider);
        Ok(())
    } else {
        log::warn!("No HSM device found. HSM features disabled.");
        Ok(())
    }
}

/// Check if HSM is available
pub fn is_available() -> bool {
    HSM_PROVIDER.lock().is_some()
}

/// Get HSM provider
pub fn get_provider() -> Option<HsmProvider> {
    HSM_PROVIDER.lock().clone()
}
