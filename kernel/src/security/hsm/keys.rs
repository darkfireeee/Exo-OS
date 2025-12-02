//! HSM Key Management
//!
//! Secure key generation, storage, and lifecycle management

use alloc::vec::Vec;

/// HSM Key Handle (opaque reference)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHandle(pub u32);

/// Key Type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Rsa2048,
    Rsa4096,
    EccP256,
    EccP384,
    EccP521,
    Aes128,
    Aes256,
}

/// Key Usage Flags
#[derive(Debug, Clone, Copy)]
pub struct KeyUsage {
    pub sign: bool,
    pub verify: bool,
    pub encrypt: bool,
    pub decrypt: bool,
    pub derive: bool,
    pub wrap: bool,
    pub unwrap: bool,
}

impl KeyUsage {
    pub const SIGNING: Self = Self {
        sign: true,
        verify: true,
        encrypt: false,
        decrypt: false,
        derive: false,
        wrap: false,
        unwrap: false,
    };

    pub const ENCRYPTION: Self = Self {
        sign: false,
        verify: false,
        encrypt: true,
        decrypt: true,
        derive: false,
        wrap: false,
        unwrap: false,
    };

    pub const ALL: Self = Self {
        sign: true,
        verify: true,
        encrypt: true,
        decrypt: true,
        derive: true,
        wrap: true,
        unwrap: true,
    };
}

/// HSM Key Object
#[derive(Debug, Clone)]
pub struct HsmKey {
    pub handle: KeyHandle,
    pub key_type: KeyType,
    pub usage: KeyUsage,
    pub extractable: bool,
    pub label: Vec<u8>,
}

/// Generate a new key in the HSM
pub fn generate_key(
    key_type: KeyType,
    usage: KeyUsage,
    extractable: bool,
    label: &[u8],
) -> Result<HsmKey, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: communicate with HSM to generate key
    // For now: return simulated key
    Ok(HsmKey {
        handle: KeyHandle(1),
        key_type,
        usage,
        extractable,
        label: label.to_vec(),
    })
}

/// Import an existing key into the HSM
pub fn import_key(
    key_type: KeyType,
    key_data: &[u8],
    usage: KeyUsage,
    label: &[u8],
) -> Result<HsmKey, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    if key_data.is_empty() {
        return Err("Invalid key data");
    }

    // In production: send key material to HSM
    Ok(HsmKey {
        handle: KeyHandle(2),
        key_type,
        usage,
        extractable: false, // Imported keys typically non-extractable
        label: label.to_vec(),
    })
}

/// Delete a key from the HSM
pub fn delete_key(handle: KeyHandle) -> Result<(), &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: send delete command to HSM
    Ok(())
}

/// Export a key (if extractable)
pub fn export_key(handle: KeyHandle) -> Result<Vec<u8>, &'static str> {
    if !super::is_available() {
        return Err("HSM not available");
    }

    // In production: check extractable flag, then export
    Err("Key not extractable")
}
