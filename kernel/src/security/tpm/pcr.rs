//! TPM Platform Configuration Registers (PCR)
//!
//! PCRs are used for system measurement and attestation.
//! TPM 2.0 typically has 24 PCRs per bank.

use super::commands::{build_pcr_extend, build_pcr_read};
use super::response::parse_pcr_read_response;
use super::{TpmError, TPM_DRIVER};
use alloc::vec::Vec;

/// PCR index (0-23 for TPM 2.0)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PcrIndex(pub u8);

impl PcrIndex {
    pub const BIOS: Self = Self(0);
    pub const UEFI_CONFIG: Self = Self(1);
    pub const BOOT_LOADER: Self = Self(4);
    pub const KERNEL: Self = Self(8);
    pub const IMA: Self = Self(10);
}

/// PCR bank (hash algorithm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcrBank {
    SHA1,
    SHA256,
    SHA384,
    SHA512,
    SM3_256,
}

impl PcrBank {
    pub fn digest_size(&self) -> usize {
        match self {
            Self::SHA1 => 20,
            Self::SHA256 => 32,
            Self::SHA384 => 48,
            Self::SHA512 => 64,
            Self::SM3_256 => 32,
        }
    }

    pub fn tpm_alg_id(&self) -> u16 {
        match self {
            Self::SHA1 => 0x0004,
            Self::SHA256 => 0x000B,
            Self::SHA384 => 0x000C,
            Self::SHA512 => 0x000D,
            Self::SM3_256 => 0x0012,
        }
    }
}

/// PCR Value (Digest)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcrValue {
    pub bank: PcrBank,
    pub value: Vec<u8>,
}

/// Extend PCR with measurement
///
/// Operation: PCR[i] = Hash(PCR[i] || data)
pub fn extend_pcr(index: PcrIndex, bank: PcrBank, data: &[u8]) -> Result<(), TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    // Hash the data first using the selected algorithm
    use crate::security::crypto::hash::sha256;

    let digest = match bank {
        PcrBank::SHA256 => sha256(data).to_vec(),
        _ => {
            // For other algorithms, use SHA256 as fallback
            // In production, implement all hash algorithms
            sha256(data).to_vec()
        }
    };

    let command = build_pcr_extend(index.0 as u32, bank.tpm_alg_id(), &digest);

    let driver = TPM_DRIVER.lock();
    let driver_ref = driver.as_ref().ok_or(TpmError::NotAvailable)?;

    let _response = driver_ref.execute(&command)?;

    Ok(())
}

/// Read PCR value
pub fn read_pcr(index: PcrIndex, bank: PcrBank) -> Result<PcrValue, TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    let command = build_pcr_read(&[index.0 as u32], bank.tpm_alg_id());

    let driver = TPM_DRIVER.lock();
    let driver_ref = driver.as_ref().ok_or(TpmError::NotAvailable)?;

    let response = driver_ref.execute(&command)?;
    drop(driver);

    let digests = parse_pcr_read_response(&response)?;

    if let Some(digest) = digests.first() {
        Ok(PcrValue {
            bank,
            value: digest.clone(),
        })
    } else {
        Err(TpmError::CommunicationError)
    }
}

/// Reset PCR (only certain PCRs can be reset, e.g., debug PCRs 16, 23)
pub fn reset_pcr(_index: PcrIndex) -> Result<(), TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    // TPM2_PCR_Reset command implementation would go here
    // Most PCRs cannot be reset, only debug PCRs

    Err(TpmError::InvalidParameter)
}

/// Quote PCRs (Attestation)
///
/// Returns a signed quote of the selected PCRs
pub fn quote_pcrs(
    _indices: &[PcrIndex],
    _bank: PcrBank,
    _nonce: &[u8],
) -> Result<Vec<u8>, TpmError> {
    if !super::is_available() {
        return Err(TpmError::NotAvailable);
    }

    // TPM2_Quote command requires an attestation key
    // This is implemented in quote.rs module

    Err(TpmError::KeyNotFound)
}
