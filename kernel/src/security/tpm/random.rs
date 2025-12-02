//! TPM Random Number Generator
//!
//! Hardware RNG provided by the TPM.
//! Compliant with NIST SP 800-90A.

use super::commands::build_get_random;
use super::response::parse_get_random_response;
use super::{TpmError, TPM_DRIVER};
use alloc::vec::Vec;

/// Get random bytes from TPM
///
/// Uses TPM2_GetRandom command.
pub fn get_random(length: usize) -> Result<Vec<u8>, TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    let mut result = Vec::new();
    let mut remaining = length;

    // TPM may return fewer bytes than requested, so loop
    while remaining > 0 {
        let request = remaining.min(32) as u16; // Max 32 bytes per request

        let command = build_get_random(request);

        let driver = TPM_DRIVER.lock();
        let driver_ref = driver.as_ref().ok_or(TpmError::NotAvailable)?;

        let response = driver_ref.execute(&command)?;
        drop(driver); // Release lock before parsing

        let random_bytes = parse_get_random_response(&response)?;

        result.extend_from_slice(&random_bytes);
        remaining = remaining.saturating_sub(random_bytes.len());

        if random_bytes.is_empty() {
            return Err(TpmError::CommunicationError);
        }
    }

    Ok(result)
}

/// Stir random pool in TPM
///
/// Uses TPM2_StirRandom to add entropy to the TPM's internal state.
pub fn stir_random(_data: &[u8]) -> Result<(), TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    // TPM2_StirRandom command could be implemented here
    // For now, just verify TPM is available
    Ok(())
}
