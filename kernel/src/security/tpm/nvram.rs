//! TPM NVRAM (Non-Volatile RAM)
//!
//! Secure storage for persistent data (keys, certificates, policies).

use super::TpmError;
use alloc::vec::Vec;

/// NV Index Handle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvIndex(pub u32);

/// NV Attributes
#[derive(Debug, Clone, Copy)]
pub struct NvAttributes {
    pub owner_write: bool,
    pub owner_read: bool,
    pub auth_read: bool,
    pub auth_write: bool,
    pub policy_read: bool,
    pub policy_write: bool,
}

impl NvAttributes {
    pub const OWNER_READ_WRITE: Self = Self {
        owner_write: true,
        owner_read: true,
        auth_read: false,
        auth_write: false,
        policy_read: false,
        policy_write: false,
    };
}

/// Define (allocate) NV space
///
/// Uses TPM2_NV_DefineSpace
pub fn nv_define_space(
    _index: NvIndex,
    _size: usize,
    _attributes: NvAttributes,
    _auth_value: Option<&[u8]>,
) -> Result<(), TpmError> {
    // In production: Send TPM2_NV_DefineSpace command
    Err(TpmError::NotAvailable)
}

/// Write to NV space
///
/// Uses TPM2_NV_Write
pub fn nv_write(_index: NvIndex, _data: &[u8], _offset: usize) -> Result<(), TpmError> {
    // In production: Send TPM2_NV_Write command
    Err(TpmError::NotAvailable)
}

/// Read from NV space
///
/// Uses TPM2_NV_Read
pub fn nv_read(_index: NvIndex, _size: usize, _offset: usize) -> Result<Vec<u8>, TpmError> {
    // In production: Send TPM2_NV_Read command
    Err(TpmError::NotAvailable)
}

/// Undefine (delete) NV space
///
/// Uses TPM2_NV_UndefineSpace
pub fn nv_undefine_space(_index: NvIndex) -> Result<(), TpmError> {
    // In production: Send TPM2_NV_UndefineSpace command
    Err(TpmError::NotAvailable)
}
