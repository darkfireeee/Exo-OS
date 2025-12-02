//! HSM Provider Interface
//!
//! Abstraction for different HSM implementations (USB tokens, PCIe cards, etc.)

use alloc::string::String;

/// HSM Provider
#[derive(Debug, Clone)]
pub struct HsmProvider {
    name: String,
    device_type: HsmDeviceType,
    capabilities: HsmCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmDeviceType {
    UsbToken,   // USB security token
    SmartCard,  // Smart card via reader
    PciCard,    // PCIe HSM adapter
    NetworkHsm, // Network-attached HSM
}

#[derive(Debug, Clone, Copy)]
pub struct HsmCapabilities {
    pub rsa_support: bool,
    pub ecc_support: bool,
    pub aes_support: bool,
    pub sha256_support: bool,
    pub secure_storage: bool,
    pub attestation: bool,
}

impl HsmProvider {
    pub fn new(name: String, device_type: HsmDeviceType, capabilities: HsmCapabilities) -> Self {
        Self {
            name,
            device_type,
            capabilities,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn device_type(&self) -> HsmDeviceType {
        self.device_type
    }

    pub fn capabilities(&self) -> &HsmCapabilities {
        &self.capabilities
    }
}

/// Detect available HSM devices
pub fn detect_hsm() -> Option<HsmProvider> {
    // Try USB tokens
    if let Some(provider) = detect_usb_token() {
        return Some(provider);
    }

    // Try PCIe cards
    if let Some(provider) = detect_pci_hsm() {
        return Some(provider);
    }

    None
}

/// Detect USB security tokens (YubiKey, etc.)
fn detect_usb_token() -> Option<HsmProvider> {
    // In production: enumerate USB devices, check VID/PID
    // For now: simulate detection failure
    None
}

/// Detect PCIe HSM cards
fn detect_pci_hsm() -> Option<HsmProvider> {
    // In production: scan PCI bus for HSM cards
    // Check for vendors: Thales, Gemalto, etc.
    None
}
