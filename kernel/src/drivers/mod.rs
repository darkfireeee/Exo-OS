//! Hardware Drivers
//!
//! This module contains all hardware drivers for Exo-OS:
//! - PCI bus enumeration
//! - Network drivers (E1000, RTL8139, VirtIO-Net)
//! - Block device drivers
//! - Character device drivers
//! - Input device drivers
//! - Video drivers

pub mod block;
pub mod char;
pub mod input;
pub mod net;
pub mod pci;
pub mod usb;
pub mod video;

/// Error type for driver operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    InitFailed,
    DeviceNotFound,
    IoError,
    NotSupported,
    Timeout,
    InvalidParameter,
    ResourceBusy,
    NoMemory,
}

/// Result type for driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Basic information about a device.
pub struct DeviceInfo {
    pub name: &'static str,
    pub vendor_id: u16,
    pub device_id: u16,
}

/// Trait that all drivers must implement.
pub trait Driver {
    /// Returns the name of the driver.
    fn name(&self) -> &str;

    /// Initializes the driver.
    fn init(&mut self) -> DriverResult<()>;

    /// Probes for the device.
    fn probe(&self) -> DriverResult<DeviceInfo>;
}

/// Initialize all hardware drivers
pub fn init() {
    log::info!("Initializing hardware drivers...");
    
    // Initialize PCI bus first (required for device detection)
    pci::init();
    
    // Initialize network drivers
    if net::e1000::init() {
        log::info!("  E1000 network driver loaded");
    }
    if net::rtl8139::init() {
        log::info!("  RTL8139 network driver loaded");
    }
    if net::virtio_net::init() {
        log::info!("  VirtIO-Net driver loaded");
    }
    
    log::info!("Hardware drivers initialized");
}
