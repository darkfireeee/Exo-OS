//! HSM Secure Storage
//!
//! Manage data objects in HSM non-volatile memory

use alloc::vec::Vec;

/// Storage Slot ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageSlot(pub u32);

impl StorageSlot {
    pub const BOOT_SECRET: Self = Self(0);
    pub const DISK_KEY: Self = Self(1);
    pub const USER_PIN: Self = Self(2);
    pub const ADMIN_PIN: Self = Self(3);
}

/// Secure Storage Interface
pub struct SecureStorage;

impl SecureStorage {
    /// Write data to secure storage slot
    pub fn write(slot: StorageSlot, data: &[u8]) -> Result<(), &'static str> {
        if !super::is_available() {
            return Err("HSM not available");
        }

        if data.len() > 4096 {
            return Err("Data too large");
        }

        // In production: write to HSM NV-RAM
        Ok(())
    }

    /// Read data from secure storage slot
    pub fn read(slot: StorageSlot) -> Result<Vec<u8>, &'static str> {
        if !super::is_available() {
            return Err("HSM not available");
        }

        // In production: read from HSM NV-RAM
        Ok(Vec::new())
    }

    /// Delete data from slot
    pub fn delete(slot: StorageSlot) -> Result<(), &'static str> {
        if !super::is_available() {
            return Err("HSM not available");
        }

        // In production: clear HSM NV-RAM slot
        Ok(())
    }

    /// Check if slot is empty
    pub fn is_empty(slot: StorageSlot) -> bool {
        Self::read(slot).map(|d| d.is_empty()).unwrap_or(true)
    }
}

/// Write to storage slot (convenience function)
pub fn write_slot(slot: StorageSlot, data: &[u8]) -> Result<(), &'static str> {
    SecureStorage::write(slot, data)
}

/// Read from storage slot (convenience function)
pub fn read_slot(slot: StorageSlot) -> Result<Vec<u8>, &'static str> {
    SecureStorage::read(slot)
}
