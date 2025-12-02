//! Trusted Platform Module (TPM) Support
//!
//! TPM 2.0 interface for hardware root of trust.
//! Provides APIs for:
//! - PCR Management (Extend/Read)
//! - Key Management (Create/Load)
//! - Random Number Generation
//! - Data Sealing/Unsealing
//! - Attestation (Quotes)

pub mod keys;
pub mod nvram;
pub mod pcr;
pub mod quote;
pub mod random;
pub mod seal;

pub mod commands;
pub mod drivers;
pub mod response;

pub use keys::{create_key, KeyType, KeyUsage, TpmKey, TpmKeyHandle};
pub use nvram::{nv_read, nv_write, NvAttributes, NvIndex};
pub use pcr::{extend_pcr, read_pcr, PcrBank, PcrIndex, PcrValue};
pub use quote::{generate_quote, AttestationData, Quote};
pub use random::{get_random, stir_random};
pub use seal::{seal_data, unseal_data, SealedData};

use drivers::TisDriver;
use spin::Mutex;

static TPM_DRIVER: Mutex<Option<TisDriver>> = Mutex::new(None);

/// Initialize TPM subsystem
pub fn init() -> Result<(), TpmError> {
    log::info!("Initializing TPM subsystem...");

    // Probe for TIS interface
    if let Some(driver) = TisDriver::probe() {
        log::info!("TPM 2.0 TIS interface found at 0xFED40000");
        *TPM_DRIVER.lock() = Some(driver);
        Ok(())
    } else {
        log::warn!("No TPM found. Security module running in software-only mode.");
        // Not an error, just optional feature disabled
        Ok(())
    }
}

/// Check if TPM is available
pub fn is_available() -> bool {
    TPM_DRIVER.lock().is_some()
}

/// TPM Error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpmError {
    NotAvailable,
    CommunicationError,
    InvalidParameter,
    AuthFailed,
    PcrMismatch,
    KeyNotFound,
    NvIndexLocked,
    CommandFailed(u32),
}

/// TPM capabilities
#[derive(Debug, Clone)]
pub struct TpmCapabilities {
    pub version: (u8, u8), // Major, minor
    pub manufacturer: u32,
    pub firmware_version: u64,
    pub max_command_size: usize,
    pub max_response_size: usize,
    pub pcr_banks: alloc::vec::Vec<PcrBank>,
}

/// Get TPM capabilities
pub fn get_capabilities() -> Result<TpmCapabilities, TpmError> {
    Err(TpmError::NotAvailable)
}
